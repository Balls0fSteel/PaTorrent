//! The English help texts (D55): a sentence or three for every Preferences control, every row of
//! the Advanced table and every main-window menu item and toolbar button, shown as tooltips, in
//! the Preferences description pane and on F1. Keys are the control's label key in [`S`]
//! (`PrefMaxConnections` beside `S::PrefMaxConnections`); an Advanced row's key is `Adv` and its
//! setting name in CamelCase (`connection_speed` is `AdvConnectionSpeed`), used where no page
//! field of the same name has a text ([`crate::help::for_setting`]).
//!
//! Each text states what the control does in this program, written from the code; the `///` note
//! before it names where (`docs/HELP_TEXTS.md` lists them side by side for review). The texts go
//! into `lang/en.lng` as `help.<Key>=` lines after the strings, and only from there into the
//! binaries, deflated (D26): the literals below are compiled into the tests alone. A translation
//! may carry `help.<Key>` lines; a help text it lacks shows in English.
//!
//! Add a text at the end of its group. Like a string's, a key is never reused for another meaning.
//!
//! [`S`]: super::S

macro_rules! helps {
    ($( $(#[doc = $note:literal])* $key:ident = $text:literal; )*) => {
        /// A help text's key; [`crate::help!`] gives its text in the current language.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum H { $($key),* }

        impl H {
            /// Every key, in table order.
            pub const ALL: &'static [H] = &[$(H::$key),*];
        }

        pub(super) const HELP_KEYS: &[&str] = &[$(stringify!($key)),*];
        /// The English texts, for the tests; the binaries inflate them from `en.lng`.
        #[cfg(test)]
        pub(super) const HELP_ENGLISH: &[&str] = &[$($text),*];
        /// Where each text's claim was read in the code (`docs/HELP_TEXTS.md`).
        #[cfg(test)]
        pub(super) const HELP_NOTES: &[&str] = &[$(concat!($($note),*)),*];
    };
}

helps! {
    // ======== Preferences pages (the Win32 dialog, GTK and the Web UI)

    // ---- Preferences > General
    /// frontend: crates/frontend/src/options.rs (GuiOptions::language, used from the next start by lang::init); nox applies it at once (crates/nox/src/webui/prefs.rs)
    PrefUiLanguage = "The language of menus, dialogs and the Web UI. The first choice follows the system's language. The desktop windows switch at their next start.";
    /// frontend: crates/win32/src/integration.rs (HKCU Software\Classes), crates/frontend/src/integration.rs (XDG entries, mimeapps.list), pref_pages.rs (greyed in a Flatpak)
    PrefAssocTorrent = "Makes this copy of PaTorrent the program that opens .torrent files, for your user account only. A program chosen in the system's own default apps settings wins over this, and only you can change that there.";
    /// frontend: crates/win32/src/integration.rs (magnet URL scheme), crates/frontend/src/integration.rs
    PrefAssocMagnet = "Makes this copy of PaTorrent the program that opens magnet: links clicked in a browser, for your user account only. With profiles, only one profile can hold them.";
    /// frontend: crates/win32/src/app.rs (check_association at start), crates/gtk/src/app.rs; options.rs: off by default
    PrefCheckAssoc = "At start, asks whether to take over .torrent files and magnet: links when PaTorrent is not the program that opens them; not when it starts minimized at login. Off by default.";
    /// frontend: crates/win32/src/integration.rs (a Run value per profile), crates/frontend/src/integration.rs (autostart entry, the Background portal in a Flatpak)
    PrefAutostart = "Starts PaTorrent when you log in to the computer. Each profile has its own entry; in a Flatpak only the default profile can start at login.";
    /// frontend: crates/frontend/src/integration.rs (the switch the autostart entry passes), crates/win32/src/app.rs (restore_placement hidden)
    PrefStartMinimized = "With the automatic start, PaTorrent starts without showing its window: in the tray where there is one, minimized where there is none.";
    /// frontend: crates/win32/src/preferences.rs (ID_DEFAULT_APPS), crates/win32/src/integration.rs (open_default_apps, UserChoice)
    PrefDefaultApps = "Opens Windows Settings at Default apps, on PaTorrent's own page where Windows has one. A program chosen there wins over the boxes above, and only you can change that choice.";
    /// frontend: crates/frontend/src/update.rs (Checker: FIRST_CHECK 10 s, INTERVAL 12 h; nothing installed without asking)
    PrefCheckUpdates = "About ten seconds after start and then every 12 hours, asks the project website whether a newer release is out and tells you. Nothing is downloaded or installed without asking. Help > Check for Updates works either way.";
    /// frontend: crates/frontend/src/update.rs (pre-releases offered with update.beta or when running one)
    PrefUpdateBeta = "Also offers pre-release (beta) versions when checking for updates. A pre-release build always hears of the next pre-release, whatever this says.";

    // ---- Preferences > UI Settings
    /// frontend: crates/win32/src/app.rs (ui.confirm_delete before Remove), crates/nox/src/webui/prefs.rs
    PrefConfirmDelete = "Asks before removing torrents from the list, saying whether their files are deleted too. Off, Remove acts at once.";
    /// frontend: crates/frontend/src/options.rs (GuiOptions::alternate_rows)
    PrefAlternateRows = "Shades every other row of the lists, so long rows are easier to follow.";
    /// frontend: crates/win32/src/app.rs (fmt::title_speed), crates/gtk/src/app.rs
    PrefTitleSpeed = "Puts the current download and upload speeds at the start of the main window's title, which also shows on the taskbar.";
    /// frontend: crates/win32/src/app.rs (tray::balloon BalloonComplete after the alerts)
    PrefBalloons = "Shows a notification at the tray icon, with the torrent's name, when a download completes.";
    /// frontend: crates/win32/src/app.rs (WM_CLOSE hides the window), crates/gtk/src/app.rs
    PrefCloseToTray = "The window's close button hides PaTorrent in the tray instead of exiting, so transfers go on. Exit from the menu or the tray icon's menu.";
    /// frontend: crates/win32/src/app.rs (WM_SIZE SIZE_MINIMIZED hides the window)
    PrefMinimizeToTray = "Minimizing hides the window in the tray instead of the taskbar. Click the tray icon to bring it back.";
    /// frontend: crates/frontend/src/options.rs (apply_to_new: PAUSED, not AUTO_MANAGED)
    PrefDontStart = "New torrents are added stopped and outside the queue, so nothing is downloaded until you start them.";
    /// frontend: crates/win32/src/app.rs (WM_COPYDATA activate_on_add), crates/gtk/src/app.rs
    PrefActivateWindow = "Brings the main window to the front when another program, such as a browser, hands PaTorrent a torrent. Off, only the Add New Torrent window comes up.";
    /// frontend: crates/win32/src/app.rs (add_magnet waits for the metadata), crates/win32/src/add.rs
    PrefShowAddDialog = "Opens the Add New Torrent window for each torrent you add, to choose the folder and the files first; for a magnet link, once its file list has arrived. Off, torrents are added at once with the defaults.";

    // ---- Preferences > Directories
    /// frontend: crates/frontend/src/options.rs (GuiOptions::download_dir)
    PrefNewDownloadsIn = "The folder new torrents are saved in, unless you choose another one when adding them.";
    /// frontend: crates/core/src/move_completed.rs (MoveCompleted on TorrentFinished, move_storage), options.rs
    PrefMoveCompleted = "When a torrent finishes downloading, its files are moved to the folder below and it keeps seeding from there. Applies to torrents that finish from now on.";
    /// frontend: crates/core/src/move_completed.rs (append_label, label_dir)
    PrefMoveAddLabel = "Moves a finished torrent that has a label into a folder named after the label, inside the folder for completed downloads. The label it has when it finishes counts.";
    /// frontend: crates/core/src/move_completed.rs (only_from = the download folder)
    PrefMoveOnlyDefault = "Moves only torrents saved in the folder for new downloads itself; torrents saved somewhere else, even in a folder inside it, stay where they are.";
    /// frontend: crates/frontend/src/torrent_folders.rs (store copies, named after the torrent)
    PrefStoreTorrents = "Keeps a copy of every added .torrent file in the folder below, named after the torrent. A magnet link's copy is stored once its metadata has arrived.";
    /// frontend: crates/frontend/src/torrent_folders.rs (moved on TorrentFinished)
    PrefMoveTorrents = "When a torrent finishes downloading, its stored .torrent copy moves from the folder above to the folder below.";
    /// frontend: crates/frontend/src/torrent_folders.rs (Poller, POLL_INTERVAL, .torrent.loaded)
    PrefAutoload = "Watches the folder below and adds each new .torrent file put there, without the Add New Torrent window, a few seconds after it stops changing. A loaded file is renamed to .torrent.loaded.";
    /// frontend: crates/frontend/src/torrent_folders.rs (autoload_delete)
    PrefAutoloadDelete = "Deletes an automatically loaded .torrent file once its torrent is added, instead of renaming it to .torrent.loaded.";

    // ---- Preferences > Connection
    /// engine: crates/frontend/src/settings.rs (set_listen_port on listen_interfaces), crates/core/src/engine/mod.rs (apply_settings, on_listen_settings_changed)
    PrefPort = "The port other peers connect to. Allow it in the firewall and forward it in the router, or let UPnP or NAT-PMP do that. A change applies at once.";
    /// frontend: crates/frontend/src/options.rs (random_port: 10000 to 60000), crates/win32/src/preferences.rs (ID_RANDOM_PORT)
    PrefRandomPort = "Fills in a random port from 10000 to 60000. It is used once you apply it.";
    /// frontend: crates/win32/src/app.rs and crates/gtk/src/app.rs (set_listen_port at start), options.rs (random_port)
    PrefRandomize = "Picks a new random listening port, from 10000 to 60000, at every start. A port forwarded by hand in the router then no longer matches.";
    /// engine: crates/core/src/engine/portmap.rs, engine/mod.rs (apply_settings starts or stops UPnP), settings.enable_upnp
    PrefUpnp = "Asks a UPnP router to forward the listening port to this computer, so peers can connect in. Turn it off when you forward the port yourself or the router misbehaves.";
    /// engine: crates/core/src/engine/portmap.rs (NAT-PMP / PCP), settings.enable_natpmp
    PrefNatPmp = "Asks the router to forward the listening port through NAT-PMP or PCP, which many routers offer besides UPnP or instead of it.";
    /// engine: crates/frontend/src/pref_pages.rs (PROXY_TYPES, proxy_from_ui), crates/core/src/settings.rs (ProxyType)
    PrefProxyType = "The kind of proxy server connections go through: SOCKS4, SOCKS5 or HTTP, which tunnels everything but plain http requests through CONNECT. (none) connects directly.";
    /// engine: crates/core/src/settings.rs (proxy_hostname)
    PrefProxy = "The proxy server's host name or IP address.";
    /// engine: crates/core/src/settings.rs (proxy_port), pref_pages.rs (check: 0 refused with a proxy)
    PrefProxyPort = "The port the proxy server listens on, such as 1080 for many SOCKS servers.";
    /// engine: crates/frontend/src/pref_pages.rs (proxy_from_ui picks Socks5Pw / HttpPw), crates/core/src/settings.rs (ProxyType::Socks4)
    PrefAuthentication = "Logs in to the proxy with the user name and password below. A SOCKS4 proxy gets only the user name, as its user id, whenever one is filled in, ticked or not.";
    /// engine: crates/core/src/settings.rs (proxy_username; SOCKS4 user id)
    PrefProxyUsername = "The user name for the proxy server. SOCKS4 sends it as its user id.";
    /// engine: crates/core/src/settings.rs (proxy_password, SOCKS5 RFC 1929 or HTTP basic authentication)
    PrefProxyPassword = "The password for the proxy server, used by SOCKS5 and HTTP proxies.";
    /// engine: crates/core/src/settings.rs (proxy_hostnames, proxy_send_host_in_connect)
    PrefProxyNames = "Lets the proxy look up the names of trackers and web seeds instead of this computer, so your own DNS server does not see them. DHT bootstrap names are then not looked up at all.";
    /// engine: crates/core/src/settings.rs (proxy_peer_connections: no listen socket, no LSD or port mapping)
    PrefProxyPeers = "Sends connections to peers and web seeds through the proxy. Incoming connections are then refused and port mapping and Local Peer Discovery stay off; uTP and the DHT need a SOCKS5 proxy.";
    /// engine: crates/core/src/settings.rs (proxy_tracker_connections)
    PrefProxyTrackers = "Sends tracker announces through the proxy, so trackers see the proxy's address instead of yours. UDP trackers need a SOCKS5 proxy; through another kind they are not reached.";
    /// engine: crates/core/src/net/bind.rs (is_vpn, match_rule), crates/core/src/engine/network_rule.rs (evaluate_network_rule, re-evaluated on every network change), settings.network_rule
    PrefNetworkRule = "Keeps all torrent traffic (peers, DHT, trackers, web seeds) on one network adapter: any adapter that looks like a VPN tunnel, one adapter by name, or whichever adapter has an address in the range below. Checked again at every network change, so a VPN that reconnects under a new address is followed. The Web UI and remote connections are not bound.";
    /// engine: crates/core/src/settings.rs (NetworkRule::Range, parse_cidr), crates/frontend/src/pref_pages.rs (check: PrefBadRange)
    PrefNetworkRange = "An address range such as 10.2.0.0/16 or fd00::/8, for the \"Address range\" choice above: the adapter holding an address in it carries the torrent traffic. An IPv4 range leaves IPv6 traffic off, and an IPv6 range IPv4. Most VPNs hand out addresses from a fixed range, which outlives renamed and recreated adapters.";
    /// engine: crates/core/src/engine/network_rule.rs (set_network_hold, session_halted), settings.network_pause_when_missing
    PrefNetworkPause = "While no adapter matches the network rule (the VPN is down), running torrents wait and show \"Waiting for the network\", then carry on by themselves when it is back. Off, nothing is paused, but no torrent traffic can leave either, since every connection needs the matched address.";

    // ---- Preferences > Bandwidth
    /// engine: crates/core/src/engine/mod.rs (apply_settings sets the global peer class), settings.upload_rate_limit
    PrefUpRate = "The most all torrents together upload per second. 0 means unlimited. The scheduler's Limited hours use their own rate in its place, and Keep ping low can lower it for a while.";
    /// engine: crates/core/src/engine/mod.rs (apply_settings sets the global peer class), settings.download_rate_limit
    PrefDownRate = "The most all torrents together download per second. 0 means unlimited. The scheduler's Limited hours use their own rate in its place, and Keep ping low can lower it for a while.";
    /// engine: crates/core/src/engine/peer_class.rs (overhead per packet), settings.rate_limit_ip_overhead
    PrefRateOverhead = "Counts the TCP/IP headers of peer traffic, estimated per packet, against the global limits, so the limits match what the line carries. Off by default.";
    /// engine: crates/core/src/engine/mod.rs (apply_rate_limit_utp), settings.rate_limit_utp
    PrefRateUtp = "Applies the global rate limits to uTP peers too. Off, uTP peers are slowed only by uTP's own delay control, which gives way to other traffic. On by default.";
    /// engine: docs/LOWPING.md (slices 1-3), crates/core/src/settings.rs (auto_rate_limit, utp_target_delay at most 25 ms)
    PrefKeepPingLow = "Measures the delay on your line with pings and uTP peers and caps uploads and downloads automatically while it grows, for games and calls. It never raises your own limits; the status bar shows the cap as auto.";
    /// engine: crates/core/src/engine/mod.rs (connect_peers_for, peer_turnover), settings.connections_limit
    PrefMaxConnections = "The most peer connections open at once across all torrents. More connections can find faster peers but load the router; lower it if the network struggles.";
    /// frontend: crates/frontend/src/options.rs (apply_to_new: max_connections), core engine/mod.rs (per torrent room)
    PrefPeersPerTorrent = "The most peers one torrent connects to; 0 means no limit. Applies to torrents added from now on.";
    /// frontend: crates/frontend/src/options.rs (apply_to_new: max_uploads, property_defaults), crates/frontend/src/properties.rs
    PrefSlotsPerTorrent = "How many peers one torrent uploads to at once; 0 means no limit. Applies to torrents added from now on; a torrent's Properties can change its own.";
    /// engine: crates/core/src/engine/mod.rs (run_choker, unchoke checks), settings.unchoke_slots_limit
    PrefGlobalSlots = "How many peers all torrents together upload to at once, the fastest ones chosen; -1 means no limit and 0 counts as 1. Fewer slots give each peer more speed.";
    /// nox: crates/nox/src/webui/prefs.rs (alt_up_limit), crates/nox/src/webui/commands.rs (in force while alt_enabled; not when hosted)
    PrefAltUpRate = "The global upload limit used instead of the normal one while the alternative rate limits are on. 0 means unlimited.";
    /// nox: crates/nox/src/webui/prefs.rs (alt_dl_limit), crates/nox/src/webui/commands.rs
    PrefAltDownRate = "The global download limit used instead of the normal one while the alternative rate limits are on. 0 means unlimited.";
    /// nox: crates/nox/src/webui/prefs.rs (alt_enabled, refused when hosted), crates/nox/src/webui/api.rs (toggleSpeedLimitsMode)
    PrefUseAltRates = "Switches to the alternative rate limits until you switch back; apps that toggle qBittorrent's speed limits mode do the same. Not available when a desktop window serves the Web UI.";

    // ---- Preferences > BitTorrent
    /// engine: crates/core/src/engine/dht.rs (private torrents excluded), settings.enable_dht
    PrefDht = "Finds peers through the DHT, a network of BitTorrent clients, without a tracker; most magnet links need it. Private torrents never use it.";
    /// engine: crates/core/src/engine/lsd.rs (private torrents excluded), settings.enable_lsd
    PrefLsd = "Finds peers for the same torrents on your local network, so they exchange data directly at network speed. Private torrents never use it.";
    /// frontend: crates/frontend/src/options.rs (DISABLE_PEX on new torrents), crates/win32/src/app.rs (set on every torrent when changed), core engine/pex.rs (private excluded)
    PrefPex = "Lets connected peers tell each other about more peers for the same torrent. A change applies to every torrent; private torrents never use it.";
    /// engine: crates/core/src/settings.rs (enable_outgoing_utp, enable_incoming_utp), crates/frontend/src/settings.rs (bt.transp_disposition)
    PrefUtp = "Connects to and accepts peers over uTP, a UDP transport that slows down when the network is busy, as well as over TCP. bt.transp_disposition on the Advanced page sets each direction separately.";
    /// engine: crates/core/src/engine/pe.rs (start_outgoing_handshake), crates/frontend/src/settings.rs (set_encryption: Forced is RC4 only)
    PrefOutgoing = "How the connections PaTorrent opens are encrypted. Disabled: never. Enabled: plain and encrypted attempts alternate per peer, keeping what worked. Forced: encrypted only, losing peers that cannot encrypt.";
    /// engine: crates/frontend/src/settings.rs (allow_legacy_incoming: in_enc_policy Enabled or Forced), crates/core/src/engine/wire.rs
    PrefLegacy = "Accepts incoming connections that are not encrypted. Off, only encrypted incoming connections are accepted, which loses peers that cannot encrypt.";
    /// frontend: crates/frontend/src/options.rs (ip_filter_file), crates/core/src/net/ip_filter.rs (formats), settings.apply_filter_to_dht / apply_ip_filter_to_trackers
    PrefIpFilterEnable = "Blocks peers whose addresses are in the block list below, in eMule ipfilter.dat or PeerGuardian P2P format; an empty path means ipfilter.dat in the settings folder. Loaded at start and when Apply changes the box or the path; Reload reads an edited list.";
    /// frontend: crates/win32/src/preferences.rs (reload_ip_filter: needs the box ticked)
    PrefReload = "Loads the block list from the path now, without waiting for Apply, and shows how many rules it holds. The IP filter must be enabled.";

    // ---- Preferences > Queueing
    /// engine: crates/core/src/engine/mod.rs (auto_manage: auto-managed torrents only), settings.active_limit
    PrefActive = "The most queued torrents running at once, downloads and seeds together; the rest wait. -1 means no limit. Forced torrents run outside the queue.";
    /// engine: crates/core/src/engine/mod.rs (auto_manage), settings.active_downloads
    PrefActiveDownloads = "The most queued torrents downloading at once; the next one in the queue starts when one finishes or stops. -1 means no limit.";
    /// engine: crates/core/src/engine/mod.rs (auto_manage), settings.active_seeds
    PrefActiveSeeds = "The most finished queued torrents seeding at once; the others wait. -1 means no limit.";
    /// engine: crates/core/src/engine/torrent.rs (is_inactive, update_inactive), settings.dont_count_slow_torrents, inactive_down_rate, inactive_up_rate, auto_manage_startup
    PrefSlow = "A torrent moving less than about 2 kB/s for a minute stops taking a download or seed slot, so another can start; it still counts toward the total. inactive_down_rate and inactive_up_rate on the Advanced page set the speeds.";
    /// frontend: crates/frontend/src/seeding.rs (SeedGoal::reached, ratio in percent), crates/core/src/status.rs (ratio)
    PrefRatio = "A queued torrent's goal: upload this percentage of what it downloaded, so 150 means one and a half times. 0 means no ratio goal; with a time goal too, both must be reached. Properties can set a torrent's own goal.";
    /// frontend: crates/frontend/src/seeding.rs (SeedGoal::reached, seeding_duration)
    PrefSeedTime = "A queued torrent's goal: seed for this many minutes. 0 means no time goal; with a ratio goal too, both must be reached.";
    /// frontend: crates/frontend/src/seeding.rs (SeedingGoal::action: LimitUpload when on and above 0, else Stop)
    PrefGoalLimit = "Instead of stopping a torrent that reached its seeding goal, caps its upload rate at this number, so it keeps seeding slowly. 0, or the box off, stops it.";
    /// nox: crates/nox/src/webui/prefs.rs (queue.seed_limit), crates/frontend/src/seeding.rs (SeedingGoal::action)
    PrefGoalLimitInstead = "A torrent that reached its seeding goal keeps seeding at the upload rate below instead of stopping. With a rate of 0 it stops anyway.";

    // ---- Preferences > Scheduler
    /// frontend: crates/frontend/src/scheduler.rs (Schedule::mode_at: Full speed while off)
    PrefScheduler = "Follows the weekly table below hour by hour, in local time. Off, PaTorrent always runs at full speed with the normal settings.";
    /// frontend: crates/frontend/src/scheduler.rs (Mode, settings_for, Change: pause session, upload mode)
    PrefSchedTable = "One cell per hour of the week; a click moves a cell on to the next of Full speed, Limited, Turn off and Seeding only, and dragging paints that mode over the cells passed. Limited uses the rates below, Turn off pauses every torrent, Seeding only uploads but downloads nothing.";
    /// frontend: crates/frontend/src/scheduler.rs (settings_for Limited: upload_rate_limit)
    PrefSchedUp = "The global upload limit during the table's Limited hours, in place of the normal one. 0 means unlimited.";
    /// frontend: crates/frontend/src/scheduler.rs (settings_for Limited: download_rate_limit)
    PrefSchedDown = "The global download limit during the table's Limited hours, in place of the normal one. 0 means unlimited.";
    /// frontend: crates/frontend/src/scheduler.rs (settings_for Off: enable_dht false)
    PrefSchedDht = "Also switches the DHT off during the table's Turn off hours, so PaTorrent stays quiet on the network. On by default.";

    // ---- Preferences > Web UI
    /// frontend: crates/win32/src/webui_host.rs (Setup::enabled, starts with the window, restarted on Apply; WebUiNeedLogin)
    PrefWebUi = "Serves the Web UI from this window, so a browser can control these torrents while PaTorrent runs. It needs a user name and a password, and restarts when you apply changes.";
    /// frontend: crates/win32/src/webui_host.rs (webui.username), crates/nox/src/webui/prefs.rs (webui.username, needs the current password)
    PrefWebUiUsername = "The user name the Web UI's login asks for. In the browser's Preferences, changing it needs the current password.";
    /// frontend: crates/win32/src/webui_host.rs (stored as a PBKDF2 hash), docs/WEBUI.md (Security: a new password ends the other sessions)
    PrefWebUiPassword = "The Web UI's login password, stored only as a hash. Leave it empty to keep the stored one. A new password logs out every other browser.";
    /// nox: crates/nox/src/webui/prefs.rs (webui.current_password, never stored), docs/WEBUI.md (Security: a wrong one ends the session)
    PrefCurrentPassword = "Your present Web UI password, needed to change the login, the API key, the subnets and the other security settings. It is never stored; a wrong one logs this browser out.";
    /// frontend: crates/win32/src/webui_host.rs (Setup::port, DEFAULT_PORT 8080, first_free_port)
    PrefWebUiPort = "The port the Web UI listens on. 8080 is proposed, or the next free port when another program or profile holds it.";
    /// frontend: crates/win32/src/webui_host.rs (Setup::addr: 0.0.0.0 or 127.0.0.1), en.rs PrefWebUiNote (firewall)
    PrefWebUiRemote = "Listens on every IPv4 address of this computer, so other computers and phones can reach the Web UI, not only this one. They also need a Windows Firewall rule for patorrent.exe.";
    /// frontend: crates/win32/src/webui_host.rs (Setup::https, webui-cert.pem), docs/WEBUI.md (HTTPS)
    PrefWebUiHttps = "Serves the Web UI over HTTPS with a certificate PaTorrent makes itself and keeps in the settings folder. Browsers warn about such a certificate until you accept it.";
    /// frontend: crates/win32/src/preferences.rs (ID_WEBUI_OPEN, enabled while running)
    PrefWebUiOpen = "Opens the running Web UI in your browser.";
    /// nox: crates/nox/src/webui/mod.rs (Prefs::api_key_enabled), crates/nox/src/webui/api.rs (Bearer check), docs/WEBUI.md (Security)
    WebApiKeyEnabled = "Lets scripts and apps such as Sonarr or Radarr use the Web UI with the key below in an Authorization: Bearer header, without logging in. Off by default.";
    /// nox: crates/nox/src/webui/auth.rs (API_KEY_MIN_LEN 16), docs/WEBUI.md (write-only, hashed, a wrong key counts as a failed login)
    WebApiKey = "The key apps send, at least 16 characters, stored only as a hash and never shown again. Leave it empty to keep the stored key. A wrong key counts as a failed login.";
    /// nox: crates/nox/src/webui/mod.rs (auth_subnets), docs/WEBUI.md (never for proxied requests)
    WebAuthSubnets = "Clients in these networks use the Web UI without logging in, with full control. Never applies to requests through a reverse proxy. List only networks you trust as much as the password.";
    /// nox: crates/nox/src/webui/mod.rs (domain_list, * wildcards), crates/nox/src/webui/api.rs (host_allowed)
    WebDomains = "Host names the Web UI answers to besides IP addresses and localhost, such as this computer's name for an app in Docker; * matches anything. While Host header validation is on, other names are refused.";
    /// nox: crates/nox/src/webui/prefs.rs (webui.listen, read-only), crates/nox/src/webui/mod.rs (--webui ADDR:PORT)
    WebListenAddress = "The address and port the Web UI listens on, shown for reference. It is set where the server starts: nox's --webui option, or a desktop window's Web UI page.";
    /// nox: crates/nox/src/webui/mod.rs (is_loopback_bypass), docs/WEBUI.md (never for proxied requests)
    WebBypassLocal = "Programs and browsers on this computer use the Web UI without logging in. Never applies to requests through a reverse proxy. Off by default.";
    /// nox: crates/nox/src/webui/api.rs (host_allowed, HOST_REFUSED), docs/WEBUI.md (Security)
    WebHostHeader = "Refuses requests addressed to any name but an IP address, localhost or a server domain listed here, which protects against web pages reaching the Web UI under a name of their own. On by default.";
    /// nox: crates/nox/src/webui/api.rs (csrf_ok: Origin or Referer must name the Host; neither passes)
    WebCsrf = "Refuses requests a browser sends from another web site's page: the Origin or Referer must be the Web UI's own address. Apps that send neither are not affected. On by default.";
    /// nox: crates/nox/src/webui/auth.rs (BanList::failed: 0 never bans, IPv6 per /64), crates/nox/src/webui/api.rs
    WebBanAfter = "After this many failed logins in a row, a client is locked out for the time below; IPv6 clients count per /64 network. 0 never bans.";
    /// nox: crates/nox/src/webui/auth.rs (BanList::failed: now + ban)
    WebBanFor = "How many seconds a client stays locked out after too many failed logins.";
    /// nox: crates/nox/src/webui/api.rs (authenticate: last_used older than the timeout)
    WebSessionTimeout = "A login ends after this many seconds without a request from that browser, which must then log in again.";

    // ---- Preferences > Advanced
    /// frontend: crates/win32/src/preferences.rs (fill_advanced), crates/frontend/src/pref_pages.rs (advanced_rows)
    PrefFilter = "Shows only the settings whose name contains this text.";
    /// frontend: crates/win32/src/preferences.rs (advanced_set, Entry::set)
    PrefValue = "The new value for the selected setting, in the setting's own unit (bytes per second for rates). Set puts it in.";
    /// frontend: crates/win32/src/preferences.rs (show_value_controls for Kind::Bool)
    PrefTrue = "Turns the selected switch on when you press Set.";
    /// frontend: crates/win32/src/preferences.rs (show_value_controls for Kind::Bool)
    PrefFalse = "Turns the selected switch off when you press Set.";
    /// frontend: crates/win32/src/preferences.rs (advanced_set; the pages follow, OK or Apply applies)
    PrefSet = "Gives the selected setting the value entered; the other pages show it too. It takes effect with OK or Apply.";
    /// frontend: crates/win32/src/preferences.rs (advanced_set reset: Settings::default)
    PrefReset = "Puts the selected setting back to its default value. It takes effect with OK or Apply.";

    // ---- Preferences > Disk Cache
    /// engine: crates/core/src/disk/mod.rs (set_read_cache), crates/core/src/settings.rs (use_read_cache, read_cache_line_size)
    PrefReadCacheEnable = "Keeps recently read pieces in memory, reading a block peers ask for together with its neighbours, which saves disk seeks when seeding. Writes are left to the system's cache.";
    /// engine: crates/frontend/src/settings.rs (set_cache_size, AUTO_CACHE_MB 32), crates/core/src/disk/mod.rs (shared by the disk threads)
    PrefCacheOverride = "Uses the size beside instead of the automatic 32 MB for the read cache, shared by the disk threads. A bigger cache can help busy seeding, at the cost of memory.";
    /// engine: crates/frontend/src/settings.rs (set_cache_expiry: off is 0), crates/core/src/settings.rs (cache_expiry)
    PrefCacheExpire = "Drops cached blocks no peer has asked for in the number of seconds beside. Off, they stay until newer blocks push them out.";
    /// engine: crates/core/src/engine/mod.rs (disk_backlogged: the larger of the setting and 4 MiB), settings.max_queued_disk_bytes
    PrefDiskWriteQueue = "How much downloaded data may wait to be written to disk; above it, PaTorrent stops reading from peers until the disk catches up. Values below 4 MB act as 4 MB.";
    /// engine: crates/core/src/disk/mod.rs (set_file_pool_size, applied at once), settings.file_pool_size
    PrefOpenFiles = "The most files PaTorrent keeps open at once; others are closed and opened again when needed. Raise it when many torrents or files are busy at the same time.";
    /// engine: crates/core/src/engine/mod.rs (Engine::new: DiskIo::new with aio_threads, not resized by apply_settings)
    PrefAioThreads = "The threads that read and write torrent data; the read cache is shared among them. Read only when PaTorrent starts.";
    /// engine: not read by the engine (crates/core/src/disk/mod.rs checks with lt-check threads, one per drive; create_torrent picks its own threads)
    PrefHashThreads = "Kept for libtorrent compatibility; this version does not use it. Checks run one per drive, set by Drives checked at once on the Disk I/O page.";

    // ---- Preferences > Disk I/O
    /// engine: docs/DISKIO.md (part 1, implementation status), crates/core/src/settings.rs (check_gently)
    PrefCheckGently = "Checks torrents at the system's background disk priority, leaves other programs' cached files alone and pauses while reads are slower than usual, so the computer stays responsive. Checks take longer while the disk is busy.";
    /// engine: crates/core/src/disk/smart.rs (Config::set, 1 MB = 1,000,000 bytes), settings.check_rate_limit, check_rate_mb
    PrefCheckRateLimit = "Caps each check's reading speed at the MB/s beside, for a disk that still lags during gentle checks. Off by default.";
    /// engine: crates/core/src/disk/smart.rs (Config::set: below 1 means all eight checking threads), docs/DISKIO.md (part 2)
    PrefActiveChecking = "How many drives are checked at the same time, one torrent per drive; torrents on the same drive wait their turn. At most eight; on the Advanced page, -1 also means eight.";
    /// engine: docs/DISKIO.md (part 3, implementation status), crates/core/src/settings.rs (tune_for_drive, drive_overrides)
    PrefTuneForDrive = "Detects whether each save folder is on an SSD, a hard disk, a removable drive or a network share, and lets a hard disk or removable drive do two disk jobs at once, a network server four. drive_overrides on the Advanced page corrects a wrong guess.";
    /// engine: docs/DISKIO.md (part 4, implementation status), crates/core/src/engine/disk_limit.rs
    PrefDiskSlowDownloads = "While a drive answers writes clearly slower than usual, halves the download rate step by step, never below 50 kB/s, and lets it grow again once the disk is quiet. It never raises your own limits.";
    /// engine: docs/DISKIO.md (part 4, implementation status), crates/core/src/engine/disk_limit.rs
    PrefDiskSlowUploads = "While a drive answers reads clearly slower than usual, halves the upload rate step by step, never below 50 kB/s, and lets it grow again once the disk is quiet. Seeding then uploads less.";

    // ---- Options > Setup Guide (D53)
    /// frontend setup_guide.rs (router_choice: OLDER_CONNECTIONS, OLDER_PER_TORRENT, OLDER_CONNECTION_SPEED; Settings connections_limit, connection_speed; GuiOptions max_peers_per_torrent)
    GuideRouter = "The older choice lowers the connections to 100 in all, 40 per new torrent and 8 new connection attempts a second, so a small router keeps up. The modern choice keeps the defaults: 200, no limit per torrent and 30 a second.";
    /// frontend setup_guide.rs (smart_disk: check_gently, tune_for_drive, disk_slow_downloads, disk_slow_uploads), docs/DISKIO.md
    GuideSmartDisk = "Turns on gentle checks, per-drive tuning and the slow-down of downloads and uploads while a disk answers slowly. It shows ticked only while all four are on; unticking it then turns them off.";

    // ======== The Advanced table: network, queue, DHT, transports

    /// engine: crates/core/src/engine/tracker.rs (TrackerManager::set_options), engine/upnp.rs, engine/portmap.rs (upnp_settings)
    AdvUserAgent = "The program name sent to HTTP trackers and to UPnP routers, which also show it as the description of the port mapping. Some private trackers only accept certain clients; otherwise it rarely needs changing.";
    /// engine: crates/core/src/engine/mod.rs (Engine::new, generate_peer_id), default PEER_ID_PREFIX
    AdvPeerFingerprint = "The prefix of the peer ID that tells other peers and trackers which client this is. A new peer ID is drawn from it at each start, so a change applies at the next start. Rarely needs changing.";
    /// engine: crates/core/src/engine/listen.rs (listen_settings_changed, open_listen_socket), default 0.0.0.0:6881,[::]:6881
    AdvListenInterfaces = "Comma-separated addresses and ports to accept peers on, written ip:port, with IPv6 addresses in square brackets. 0.0.0.0 and [::] mean every interface. The listen sockets reopen at once when it changes.";
    /// engine: crates/core/src/engine/dht.rs (Engine::start_dht, bootstrap_lookups), default dht.libtorrent.org:25401
    AdvDhtBootstrapNodes = "Comma-separated host:port list of DHT routers used to join the DHT when the routing table is empty. Read when the DHT starts. Through a proxy that cannot resolve names, only entries given as IP addresses are used.";
    /// engine: crates/core/src/engine/auto_limit.rs (Engine::auto_limit step, allowance), default 15
    AdvAutoLimitDelayMs = "Milliseconds of extra ping the automatic rate limit allows above the line's idle ping before it lowers the speed caps. Lower keeps games and calls smoother; higher allows more speed. Only used while the automatic limit is on.";
    /// engine: crates/core/src/engine/auto_limit.rs (Prober::start with probe::parse_hosts), default 1.1.1.1,8.8.8.8,9.9.9.9
    AdvAutoLimitProbeHosts = "Comma-separated IP addresses the automatic rate limit pings to measure the line's delay when no router past the line answers. Read when the automatic limit is switched on. Rarely needs changing.";
    /// engine: crates/core/src/engine/mod.rs (AnnounceSlots::new, auto_manage), default 88
    AdvActiveDhtLimit = "How many running auto-managed torrents announce themselves on the DHT, taken in queue order with downloads first. Torrents beyond it still run but are not announced there. -1 means no limit.";
    /// engine: crates/core/src/engine/mod.rs (AnnounceSlots::new, auto_manage), default 1600
    AdvActiveTrackerLimit = "How many running auto-managed torrents announce to their trackers, taken in queue order with downloads first. Torrents beyond it only send a final stopped announce. -1 means no limit.";
    /// engine: crates/core/src/engine/mod.rs (AnnounceSlots::new, auto_manage), default 60
    AdvActiveLsdLimit = "How many running auto-managed torrents are announced on the local network through Local Peer Discovery, taken in queue order with downloads first. -1 means no limit.";
    /// engine: crates/core/src/engine/torrent.rs (Torrent inactive check, settle_inactive), default 2048
    AdvInactiveDownRate = "Download speed in bytes per second below which a downloading torrent counts as inactive. With slow torrents not counted, an inactive torrent does not take a download slot in the queue. Default 2048.";
    /// engine: crates/core/src/engine/torrent.rs (Torrent inactive check, settle_inactive), default 2048
    AdvInactiveUpRate = "Upload speed in bytes per second below which a finished torrent counts as inactive. With slow torrents not counted, an inactive torrent does not take a seeding slot in the queue. Default 2048.";
    /// engine: crates/core/src/engine/torrent.rs (settle_inactive delay), default 60
    AdvAutoManageStartup = "Seconds a torrent must stay below the inactive speed before it counts as inactive and frees its queue slot. Speeding up again makes it active at once. Default 60.";
    /// engine: crates/core/src/engine/mod.rs (Engine::tick, auto_manage), default 30
    AdvAutoManageInterval = "Seconds between regular checks of the queue that decide which auto-managed torrents run. Changes such as adding a torrent trigger a check at once anyway. Default 30; rarely needs changing.";
    /// engine: crates/core/src/engine/mod.rs (Engine::tick, connect_budget), default 30
    AdvConnectionSpeed = "New outgoing peer connection attempts per second, shared by all torrents. Higher finds peers faster but can overload weak routers; 0 stops new outgoing connections. Default 30.";
    /// engine: crates/core/src/engine/listen.rs (open_listen_socket), default 10
    AdvMaxRetryPortBind = "When the listening port is busy, how many following ports are tried in turn before giving up. 0 tries only the configured port. Default 10.";
    /// engine: crates/core/src/engine/ip_filter.rs (Engine::add_peer), engine/peer_list.rs, default 3000
    AdvMaxPeerlistSize = "Most peer addresses remembered per running torrent, connected or not. When the list is full, peers that failed are dropped first to make room. Default 3000; raise it for very large swarms.";
    /// engine: crates/core/src/engine/ip_filter.rs (set_privileged_ports, update_privileged_ports)
    AdvNoConnectPrivilegedPorts = "When on, peers on ports below 1024 are never added or connected to, and are removed from peer lists at once. This keeps trackers or other peers from aiming connections at web, mail and similar services. Off by default.";
    /// engine: crates/core/src/engine/peer_list.rs (Reconnect::from, connect candidates), default 3
    AdvMaxFailcount = "How many failed connection attempts in a row before a peer is no longer tried. A tracker listing the peer again takes one failure off. Default 3.";
    /// engine: crates/core/src/engine/peer_list.rs (Reconnect, connect candidates), default 60
    AdvMinReconnectTime = "Seconds to wait after a connection to a peer closes before trying it again, multiplied by its number of failures plus one. Default 60.";
    /// engine: crates/core/src/engine/mod.rs (stopping a torrent, PeerList::trim), default 1000
    AdvMaxPausedPeerlistSize = "Most unconnected peer addresses kept for a torrent when it is stopped or paused; the rest are dropped. Default 1000.";
    /// engine: crates/core/src/engine/dht.rs (update_dht_announce_interval), default 900
    AdvDhtAnnounceInterval = "Seconds in which every running torrent is announced on the DHT once; the announces are spread evenly over that time. New torrents announce within a few seconds anyway. Default 900 (15 minutes).";
    /// engine: crates/core/src/dht/mod.rs (DhtIo send quota), default 8000
    AdvDhtUploadRateLimit = "Bytes per second the DHT may send; once it is used up, other nodes' queries go unanswered until it refills. Raise it if the DHT seems slow on a fast line; lower it on a very slow upload. Default 8000.";
    /// engine: crates/core/src/dht/node.rs (read_only checks), default off
    AdvDhtReadOnly = "When on, the DHT looks up peers but does not answer other nodes' queries, and marks its own queries read-only so others do not add it to their tables. Saves upload; off by default.";
    /// engine: crates/core/src/dht/node.rs (get_peers obfuscated lookup), default off
    AdvDhtPrivacyLookups = "When on, DHT lookups hide which torrent is looked for from most of the nodes asked, at the cost of slightly slower lookups. Off by default.";
    /// engine: crates/core/src/dht/node.rs, dht/routing_table.rs (verify_id), default off
    AdvDhtEnforceNodeId = "When on, DHT nodes whose ID does not match their IP address are ignored, which makes the routing table harder to poison. Many nodes do not follow this rule yet, so it is off by default.";
    /// engine: crates/core/src/dht/routing_table.rs (add node), default on
    AdvDhtRestrictRoutingIps = "When on, the DHT routing table keeps one entry per IP address and no two close-by addresses in one bucket. This makes it harder for one party to fill the table. On by default.";
    /// engine: crates/core/src/dht/node.rs (lookup results), default on
    AdvDhtRestrictSearchIps = "When on, a DHT lookup takes at most one result from each block of close-by addresses, so one party cannot steer the lookup. On by default.";
    /// engine: crates/core/src/dht/routing_table.rs (bucket split), default on
    AdvDhtPreferVerifiedNodeIds = "When on, only DHT nodes whose ID matches their IP address may cause the routing table to grow, so verified nodes are preferred. On by default.";
    /// engine: crates/core/src/dht/routing_table.rs (bucket size), default on
    AdvDhtExtendedRoutingTable = "When on, the first buckets of the DHT routing table hold more nodes, which makes lookups faster at a small memory cost. On by default.";
    /// engine: crates/core/src/dht/node.rs (traversal, aggressive_lookups), default on
    AdvDhtAggressiveLookups = "When on, DHT lookups keep several queries outstanding to the best nodes found so far, which finishes them faster for a little more traffic. On by default.";
    /// engine: crates/core/src/dht/mod.rs (incoming packet check), default on
    AdvDhtIgnoreDarkInternet = "When on, DHT packets from IPv4 address ranges that are not used on the public internet are ignored, as they are likely forged. On by default.";
    /// engine: crates/core/src/dht/storage.rs (get_peers reply), default 100
    AdvDhtMaxPeersReply = "Most peers this client lists in one answer to another node asking the DHT for a torrent's peers. Default 100; rarely needs changing.";
    /// engine: crates/core/src/dht/storage.rs (sample_infohashes), default 21600
    AdvDhtSampleInfohashesInterval = "Seconds a sample of stored torrent hashes is reused when other nodes ask for one, at most 21600 (6 hours). Used by DHT indexers; rarely needs changing.";
    /// engine: crates/core/src/dht/storage.rs (sample_infohashes), default 20
    AdvDhtMaxInfohashesSampleCount = "Most stored torrent hashes given in one sample when another node asks for one, at most 20. Used by DHT indexers; rarely needs changing.";
    /// engine: crates/core/src/dht/node.rs (traversal branch_factor), default 5
    AdvDhtSearchBranching = "How many DHT nodes a lookup queries at the same time. Higher finds peers faster for more traffic; at least 1. Default 5.";
    /// engine: crates/core/src/dht/routing_table.rs (RoutingTable::node_failed), default 20
    AdvDhtMaxFailCount = "How many unanswered queries in a row before a node leaves the DHT routing table, when no spare node waits to take its place; with a spare, one failure is enough. Default 20; rarely needs changing.";
    /// engine: crates/core/src/dht/storage.rs (announce_peer storage), default 2000
    AdvDhtMaxTorrents = "Most torrents this client stores peer lists for on behalf of other DHT nodes. Default 2000; lower it to save memory.";
    /// engine: crates/core/src/dht/storage.rs (announce_peer storage), default 500
    AdvDhtMaxPeers = "Most peers stored per torrent on behalf of other DHT nodes. Default 500; lower it to save memory.";
    /// engine: crates/core/src/dht/dos_blocker.rs (DosBlocker::incoming), default 300
    AdvDhtBlockTimeout = "Seconds an IP address that floods the DHT with messages stays ignored, counted from when it was blocked. Default 300 (5 minutes).";
    /// engine: crates/core/src/dht/dos_blocker.rs (DosBlocker::incoming), default 5
    AdvDhtBlockRatelimit = "DHT messages per second, averaged over 10 seconds, one IP address may send before it is ignored for the block timeout. Default 5.";
    /// engine: crates/core/src/dht/mod.rs (incoming packet check), dht/node.rs, dht/routing_table.rs, default on
    AdvApplyFilterToDht = "When on, the IP filter also applies to the DHT: blocked addresses are ignored and kept out of the routing table and lookups. Nodes already in the table stay; bootstrap routers are exempt. On by default.";
    /// engine: crates/core/src/engine/lsd.rs (update_interval), default 300
    AdvLocalServiceAnnounceInterval = "Seconds in which every running torrent is announced once on the local network through Local Peer Discovery, spread evenly over that time. Default 300 (5 minutes).";
    /// engine: crates/core/src/engine/upnp.rs (on_reply), default off
    AdvUpnpIgnoreNonrouters = "When on, UPnP replies are only taken from devices on the listening address's own subnet, so other devices on a large network cannot answer for the router. Off by default.";
    /// engine: crates/core/src/engine/upnp.rs (mapping request, renewal at three quarters), default 3600
    AdvUpnpLeaseDuration = "Seconds a UPnP port mapping is asked for; it is renewed before it runs out. 0 asks for a permanent mapping, which some routers need. Default 3600.";
    /// engine: crates/core/src/engine/portmap.rs (natpmp_gateway), default empty
    AdvNatpmpGateway = "The IP address of the router to ask for NAT-PMP and PCP port mappings, instead of the default gateway from the routing table. Empty uses the default gateway. Read when NAT-PMP starts.";
    /// engine: crates/core/src/engine/natpmp.rs (lease lifetime), engine/portmap.rs (update_upnp_settings), default 3600
    AdvNatpmpLeaseDuration = "Seconds a NAT-PMP or PCP port mapping is asked for; it is renewed before it runs out. 0 means 3600 (one hour).";
    /// engine: crates/core/src/engine/mod.rs (incoming TCP accept), engine/listen.rs, default on
    AdvEnableIncomingTcp = "Accept incoming peer connections over TCP. When off, no TCP listening socket is opened. On by default.";
    /// engine: crates/core/src/engine/mod.rs (connect_to_peer), default on
    AdvEnableOutgoingTcp = "Make outgoing peer connections over TCP. With both outgoing TCP and uTP off, no outgoing connections are made. On by default.";
    /// engine: crates/core/src/utp/mod.rs (UtpSettings), engine/mod.rs, engine/dht.rs (dht_announce), default on
    AdvEnableIncomingUtp = "Accept incoming peer connections over uTP, the UDP-based protocol that yields to other traffic. When off, incoming uTP connections are refused. On by default.";
    /// engine: crates/core/src/engine/mod.rs (connect_to_peer), default on
    AdvEnableOutgoingUtp = "Make outgoing peer connections over uTP, the UDP-based protocol that yields to other traffic. On by default.";
    /// engine: crates/core/src/engine/peer_class.rs (update_mixed_mode), default 1
    AdvMixedModeAlgorithm = "How TCP peers are limited while uTP peers are busy. 1 limits TCP peers to four times their share of the speed by peer count, so uTP is not crowded out; 0 never limits TCP for uTP's sake. Default 1.";
    /// engine: crates/core/src/utp/mod.rs (UtpSettings::from), utp/socket.rs, default 100
    AdvUtpTargetDelay = "Milliseconds of queueing delay uTP connections aim for before they slow down. Lower keeps ping lower but may reduce speed. While the automatic rate limit is on, at most 25 is used. Default 100.";
    /// engine: crates/core/src/utp/socket.rs (congestion window growth), default 3000
    AdvUtpGainFactor = "Bytes the uTP sending window may grow per round trip when there is no queueing delay. Higher speeds up faster but is less gentle on other traffic. Default 3000; rarely needs changing.";
    /// engine: crates/core/src/utp/socket.rs (packet timeout), default 500
    AdvUtpMinTimeout = "Shortest time in milliseconds uTP waits for an acknowledgement before it treats a packet as lost. Default 500; rarely needs changing.";
    /// engine: crates/core/src/utp/socket.rs (resend checks), default 2
    AdvUtpSynResends = "Kept for libtorrent compatibility; it has no effect here. A uTP connection request is sent once and given up after the connect timeout. Default 2.";
    /// engine: crates/core/src/utp/socket.rs (resend checks), default 2
    AdvUtpFinResends = "How many times the closing packet of a uTP connection is sent and times out before the connection is dropped. Default 2.";
    /// engine: crates/core/src/utp/socket.rs (resend checks, timeouts), default 3
    AdvUtpNumResends = "How many times a uTP packet is sent and times out before the connection is dropped. Default 3.";
    /// engine: crates/core/src/utp/socket.rs (connect timeout), default 3000
    AdvUtpConnectTimeout = "Milliseconds to wait for an answer to a uTP connection request before the attempt is given up. Default 3000.";
    /// engine: crates/core/src/utp/socket.rs (on loss, window cut), default 50
    AdvUtpLossMultiplier = "Percent of the uTP sending window kept when a packet is lost. Lower backs off harder on a lossy line. Default 50.";
    /// engine: crates/core/src/utp/socket.rs (on loss, next_loss), default 100
    AdvUtpCwndReduceTimer = "Milliseconds after a cut of the uTP sending window during which further losses do not cut it again. Default 100; rarely needs changing.";

    // ======== The Advanced table: encryption, trackers, peers, disk, web seeds, proxy

    /// engine: crates/core/src/engine/pe.rs (Engine::start_outgoing_handshake), numbers in frontend policy_to_int, default 1
    AdvOutEncPolicy = "Encryption of connections PaTorrent makes. 0 forced: always encrypted. 1 enabled, the default: plain first, encrypted if that fails, remembering what worked for each peer. 2 disabled: never encrypted. Change it if your provider throttles BitTorrent traffic.";
    /// engine: crates/core/src/engine/wire.rs (Engine::on_handshake), default 1
    AdvInEncPolicy = "Which incoming connections are accepted. 0 forced: only encrypted ones, plain ones are refused. 1 enabled, the default: both. 2 disabled: only plain ones, encrypted ones are refused. Leave at 1 unless you need to refuse one kind.";
    /// engine: crates/core/src/proto/pe_handshake.rs (PeHandshake, level_bits), numbers in frontend level_to_int, default 3
    AdvAllowedEncLevel = "How much of an encrypted connection is encrypted. 1: only the handshake is hidden and the data then flows plain. 2: everything is RC4 encrypted. 3, the default: either; on connections PaTorrent makes the other side chooses, on incoming ones only the handshake is hidden unless prefer_rc4 is on. Only matters when encryption is used.";
    /// engine: crates/core/src/proto/pe_handshake.rs (incoming crypto selection), engine/wire.rs (on_handshake), default false
    AdvPreferRc4 = "When an incoming encrypted connection offers both levels, choose full RC4 encryption instead of hiding only the handshake. Off by default, which saves processor time; turn it on if handshake-only encryption is still being throttled.";
    /// engine: crates/core/src/engine/mod.rs (shutdown, Trackers phase), default 5
    AdvStopTrackerTimeout = "Seconds PaTorrent waits on exit for trackers to receive the 'stopped' message and for routers to remove port mappings. Default 5. Raise it for slow trackers, lower it to exit faster.";
    /// engine: crates/core/src/engine/tracker.rs (TrackerManager::set_options, request deadline), default 30, minimum 1
    AdvTrackerCompletionTimeout = "Seconds a tracker request, HTTP or UDP, may take in total before it counts as failed. Default 30; values below 1 count as 1. Rarely needs changing.";
    /// engine: none; field only in crates/core/src/settings.rs, stored but never read, default 10
    AdvTrackerReceiveTimeout = "Kept for compatibility; this engine does not use it yet. Tracker requests are limited by tracker_completion_timeout instead.";
    /// engine: crates/core/src/engine/tracker.rs (TrackerEntry::announced), default 300
    AdvMinAnnounceInterval = "Seconds; only affects trackers that ask to be contacted again within less than a minute. Their wait is raised to this value, at most 60, so the default 300 means one minute. Longer tracker intervals are used as given.";
    /// engine: crates/core/src/engine/mod.rs (Engine::announce), default 200
    AdvNumWant = "How many peers each announce asks the tracker for. Default 200. A finished torrent that already has more than 50 peers asks for none. Lower it to reduce connection attempts.";
    /// engine: crates/core/src/engine/tracker.rs (trackers_to_announce), default true
    AdvAnnounceToAllTiers = "On: one working tracker in every tier of a torrent's tracker list is contacted. Off: only the first tier that has a working tracker. On by default, which finds more peers.";
    /// engine: crates/core/src/engine/tracker.rs (trackers_to_announce), default false
    AdvAnnounceToAllTrackers = "On: every tracker in a tier is contacted, not just the first working one. Off by default. Turning it on can find more peers at the cost of more tracker traffic.";
    /// engine: crates/core/src/engine/ip_filter.rs (tracker filter), default true
    AdvApplyIpFilterToTrackers = "The IP filter also stops PaTorrent from contacting trackers at blocked addresses, for torrents the filter applies to. On by default; turn it off if a needed tracker sits in a blocked range.";
    /// engine: none; field only in crates/core/src/settings.rs, stored but never read, default 60
    AdvRequestTimeout = "Kept for compatibility; this engine does not use it yet. Slow block requests are handled by a fixed timeout.";
    /// engine: crates/core/src/engine/smart_ban.rs (Trust::failed), engine/picker.rs (on_parole picking), default true
    AdvUseParoleMode = "A peer that sent part of a piece that failed its check may then only download whole pieces alone until one passes, so the next bad piece is clearly its fault. On by default; helps catch peers sending bad data.";
    /// engine: crates/core/src/engine/mod.rs (torrent tick, PeerState::Active), default 120
    AdvPeerTimeout = "Seconds a connected peer may stay completely silent before it is disconnected. Default 120; normal peers send keep-alives more often than that. Rarely needs changing.";
    /// engine: crates/core/src/engine/mod.rs (torrent tick, PeerState::Connecting), engine/web_seed.rs (web_tick), default 15
    AdvPeerConnectTimeout = "Seconds an outgoing connection to a peer or web seed may take to be established before it is given up. Default 15. Raise it on very slow or high-latency networks.";
    /// engine: crates/core/src/engine/mod.rs (torrent tick, PeerState::Handshake), default 10
    AdvHandshakeTimeout = "Seconds a new connection may take to finish the BitTorrent handshake before it is closed. Default 10. Rarely needs changing.";
    /// engine: crates/core/src/engine/wire.rs (request_blocks), engine/peer.rs (Peer::desired_queue), default 3
    AdvRequestQueueTime = "Seconds of downloading, at a peer's current speed, to keep requested from it in advance; at least 4 blocks. Default 3. Raise it on high-latency links if fast peers do not reach full speed.";
    /// engine: crates/core/src/engine/wire.rs (request_blocks), engine/web_seed.rs (web_request_blocks), default 500
    AdvMaxOutRequestQueue = "Most 16 KiB block requests outstanding to one peer or web seed at a time, also capped by what the peer allows. Default 500. Rarely needs changing.";
    /// engine: crates/core/src/engine/wire.rs (on_request, extension handshake reqq), default 500
    AdvMaxAllowedInRequestQueue = "Most block requests from one peer queued for upload at a time; PaTorrent tells peers this number, and requests beyond it are rejected, or ignored for peers without the fast extension. Default 500. Rarely needs changing.";
    /// engine: crates/core/src/engine/mod.rs (session tick, run_choker), default 15, minimum 1
    AdvUnchokeInterval = "Seconds between decisions about which peers PaTorrent uploads to. Default 15. Shorter reacts faster to changing peer speeds but swaps peers more often.";
    /// engine: crates/core/src/engine/mod.rs (session tick, run_choker optimistic), default 30, minimum 1
    AdvOptimisticUnchokeInterval = "Seconds between choosing a new random peer to upload to, giving newcomers a chance. It is checked when the regular unchoke runs, so it rounds up to a multiple of unchoke_interval. Default 30.";
    /// engine: crates/core/src/engine/wire.rs (pump_uploads, on_request), engine/mod.rs (max_send_buffer), default 512000
    AdvSendBufferWatermark = "Bytes waiting to be sent to a peer before PaTorrent stops reading more data for it from disk. Default 500 KiB. A peer leaving four times this, at least 4 MiB, unread is disconnected. Raise it for very fast uploads.";
    /// engine: crates/core/src/engine/mod.rs (on_finished), engine/wire.rs (Have, set_peer_pieces), default true
    AdvCloseRedundantConnections = "Disconnects peers that can no longer exchange anything with you, such as other seeds once your download is complete. On by default, which frees connection slots for useful peers.";
    /// engine: crates/core/src/engine/mod.rs (Engine::peer_turnover), default 4
    AdvPeerTurnover = "Percent of a torrent's peers dropped at each turnover, the least useful first, to make room for possibly better ones; at least one, but no more than it has other peers to try. Default 4. 0 still drops one peer; turnover is off when the connection limit is 5 or less.";
    /// engine: crates/core/src/engine/mod.rs (Engine::peer_turnover), default 90
    AdvPeerTurnoverCutoff = "Percent of the connection limit that must be in use before peer turnover drops any peers. Default 90. Applies to the global limit and to each torrent's own limit.";
    /// engine: crates/core/src/engine/mod.rs (session tick, next_turnover), default 300, minimum 1
    AdvPeerTurnoverInterval = "Seconds between peer turnover runs, which drop a few of the least useful peers when connections are nearly full. Default 300. Rarely needs changing.";
    /// engine: crates/core/src/net/sys.rs (set_socket_buffer_size), engine/mod.rs (update_socket_buffer_size), default 0
    AdvRecvSocketBufferSize = "Receive buffer size in bytes for peer, listen and UDP tracker sockets. 0, the default, keeps the system's size. Applies to the listen sockets at once and to new peer connections; existing connections keep theirs. Going back to 0 takes full effect after a restart.";
    /// engine: crates/core/src/net/sys.rs (set_socket_buffer_size), engine/mod.rs (update_socket_buffer_size), default 0
    AdvSendSocketBufferSize = "Send buffer size in bytes for peer, listen and UDP tracker sockets. 0, the default, keeps the system's size. Applies to the listen sockets at once and to new peer connections; existing connections keep theirs. Going back to 0 takes full effect after a restart.";
    /// engine: crates/core/src/engine/metadata.rs (on_metadata_message), default 31457280
    AdvMaxMetadataSize = "Largest torrent information, in bytes, accepted from peers when starting from a magnet link. Default 30 MiB. Raise it only for magnet links of torrents with an extremely large number of files.";
    /// engine: crates/core/src/engine/pex.rs (on_pex_message), proto/ut_pex.rs (on_message), default 50
    AdvMaxPexPeers = "Most peers remembered from one connection's peer exchange messages, counted separately for IPv4 and IPv6. Default 50. Limits how many addresses a single peer can feed you.";
    /// engine: crates/core/src/alert.rs (AlertQueue), session.rs (set_limit), default 2000
    AdvAlertQueueSize = "Most engine events waiting for the window to collect them; important ones may use two or three times this. Extra events are dropped and the loss is reported. Default 2000. Raise it if the log reports dropped events.";
    /// engine: crates/core/src/alert.rs (AlertCategory, masked_by), engine/mod.rs (post alert), default 550239
    AdvAlertMask = "Bit mask of the event categories the engine reports, such as 1 errors, 16 trackers, 32 peer connects, 64 status. The default 550239 is everything except peer connects. Clearing bits can leave parts of the window or log without updates.";
    /// engine: crates/core/src/engine/mod.rs (event loop tick), default 500, minimum 10
    AdvTickInterval = "Milliseconds between the engine's regular housekeeping rounds, which handle timeouts, choking and status. Default 500; values below 10 count as 10. Rarely needs changing.";
    /// engine: crates/core/src/disk/mod.rs (DiskIo::set_read_cache), default -1
    AdvCacheSize = "Size of the disk read cache in 16 KiB blocks, shared by the disk threads. -1, the default, is automatic: 2048 blocks, 32 MiB. 0 turns the cache off, and so does a size too small to give each disk thread one read_cache_line_size line. Raise it to cut disk seeks when seeding many torrents.";
    /// engine: crates/core/src/disk/mod.rs (set_read_cache), disk/read_cache.rs, default 32
    AdvReadCacheLineSize = "How many 16 KiB blocks are read from disk at once when a peer asks for one, keeping the rest in the cache. Default 32, 512 KiB. 0 turns the read cache off.";
    /// engine: crates/core/src/disk/read_cache.rs (expiry), disk/mod.rs (set_read_cache), default 300
    AdvCacheExpiry = "Seconds an unused piece of the read cache is kept before it is dropped. 0 keeps it until the space is needed. Default 300.";
    /// engine: crates/core/src/disk/drive.rs (overridden), engine/mod.rs (set_smart_disk), default empty
    AdvDriveOverrides = "Corrects drives whose type is detected wrongly: comma-separated path=type pairs, the type one of ssd, hdd, network or removable, such as 'D:/=hdd, /mnt/nas=network'. The longest matching path wins; malformed pairs are ignored.";
    /// engine: crates/core/src/engine/web_seed.rs (web_request_blocks), default 5
    AdvUrlseedPipelineSize = "HTTP requests kept in flight on one web seed connection; one when the server closes the connection after each answer. Default 5. Rarely needs changing.";
    /// engine: crates/core/src/engine/web_seed.rs (web_tick), default 20, minimum 1
    AdvUrlseedTimeout = "Seconds a web seed connection may go without sending data before it is closed. Default 20. Raise it for slow web servers.";
    /// engine: crates/core/src/engine/web_seed.rs (retry after failure or IP filter block), default 30
    AdvUrlseedWaitRetry = "Seconds before a web seed that failed, timed out or was blocked is tried again; a Retry-After from the server takes precedence. Default 30.";
    /// engine: crates/core/src/engine/web_seed.rs (web_request_blocks), default 16777216, minimum 16384
    AdvUrlseedMaxRequestBytes = "Largest single HTTP range request sent to a web seed, in bytes. Default 16 MiB; at least 16 KiB is used. Lower it if a web server rejects or stalls on large requests.";
    /// engine: crates/core/src/engine/web_seed.rs (web seed name resolved), default 1800
    AdvWebSeedNameLookupRetry = "Seconds before a web seed whose host name could not be looked up is tried again. Default 1800, half an hour.";
    /// engine: crates/core/src/engine/web_seed.rs (maybe_connect_web_seeds), default 3
    AdvMaxWebSeedConnections = "Web seeds connected at once per downloading torrent; they count toward the connection limits. Default 3; 0 means 100. Raise it for torrents with many web seeds.";
    /// engine: crates/core/src/engine/web_seed.rs (web_seed_hash_failed), default true
    AdvBanWebSeeds = "A web seed that sends data failing the hash check can be banned like a peer, so it is no longer used. On by default; turn it off to keep using a web seed you trust.";
    /// engine: crates/core/src/engine/web_seed.rs (connect, web_redirect), engine/tracker.rs (loopback /announce path, drop_this_host_peers), default true
    AdvSsrfMitigation = "Stops torrents from using PaTorrent to reach your local services: local web seeds may not use query strings, internet web seeds may not redirect to the local network, and trackers on this machine only get /announce requests. On by default; keep it on.";
    /// engine: crates/core/src/engine/web_seed.rs (connect_to_url_seed, is_idna), default false
    AdvAllowIdna = "Allows web seeds whose host names use international characters, the xn-- form. Off by default, since such names can imitate other sites; those web seeds are skipped.";
    /// engine: crates/core/src/proto/web_peer.rs (request, first_request), default false
    AdvAlwaysSendUserAgent = "Sends the User-Agent with every web seed request instead of only the first on each connection. Off by default; turn it on if a web server requires it on every request.";
    /// engine: crates/core/src/engine/proxy.rs (ProxyConfig::from_settings), numbers in frontend proxy_to_int, default 0
    AdvProxyType = "The proxy server type. 0 none, the default. 1 SOCKS4. 2 SOCKS5. 3 SOCKS5 with user name and password. 4 HTTP. 5 HTTP with user name and password. Takes effect at once.";
    /// engine: crates/core/src/engine/proxy.rs (ProxyConfig::resolves_names), default true
    AdvProxySendHostInConnect = "With an HTTP proxy and proxy host name resolving on, CONNECT requests name the host so the proxy looks it up itself, keeping names off your local DNS. Plain http:// requests always carry the name. On by default; turn it off if the proxy cannot resolve names.";
    /// engine: crates/core/src/engine/proxy.rs (SOCKS5 UDP ASSOCIATE target), default false
    AdvSocks5UdpSendLocalEp = "Tells a SOCKS5 proxy your UDP address and port when setting up UDP, so it can send to you first. Off by default; turn it on only if the proxy needs it and no router translates addresses in between.";
    /// engine: crates/frontend/src/settings.rs (transp_disposition), engine enable_incoming/outgoing_tcp/utp, default 15
    AdvBtTranspDisposition = "Allowed peer connection types as bits added together: 1 outgoing TCP, 2 outgoing uTP, 4 incoming TCP, 8 incoming uTP. Default 15 allows all. Clear a bit to stop that kind, for example 5 for TCP only.";

    // ======== Menus and toolbar

    // ---- Menu: bar
    /// win32 app.rs main_menu -> submenu(bar, file, MenuFile)
    MenuFile = "Adds and creates torrents, imports them from other clients, switches profiles, connects to a remote server and quits PaTorrent.";
    /// win32 app.rs main_menu -> submenu(bar, options, MenuOptions)
    MenuOptions = "Opens Preferences, the Setup Guide and the RSS Downloader, shows or hides parts of the window, and sets Auto Shutdown and the tray behaviour.";
    /// win32 app.rs main_menu -> submenu(bar, help, MenuHelp)
    MenuHelp = "Opens the PaTorrent website, checks for updates, and shows the version and the third-party licence texts.";

    // ---- Menu: File
    /// win32 app.rs cmd::ADD -> dialogs::open_torrents, add_sources -> add_torrent_file
    MenuAddTorrent = "Picks one or more .torrent files and adds them, through the Add New Torrent dialog when Preferences has it turned on.";
    /// win32 app.rs cmd::ADD_URL -> dialogs::input (prefilled from the clipboard), add_sources -> add_magnet or download_url
    MenuAddUrl = "Asks for a magnet link or an http or https address of a .torrent file and adds it. A link on the clipboard is filled in.";
    /// win32 app.rs cmd::CREATE -> create_torrent -> create::show, create::seed_params
    MenuCreate = "Makes a new .torrent file from a file or folder on this computer, and can start seeding it at once.";
    /// win32 app.rs cmd::IMPORT_BASE -> import_from(Client::QBittorrent) -> frontend import::begin, import::finish
    MenuImportQbt = "Adds the torrents of qBittorrent on this computer with their data, statistics, trackers and labels. qBittorrent must not be running; the files are checked.";
    /// win32 app.rs cmd::IMPORT_BASE -> import_from(Client::UTorrent) -> frontend import::begin, import::finish
    MenuImportUt = "Adds the torrents listed in µTorrent's resume.dat with their data, statistics, trackers and labels. µTorrent must not be running; the files are checked.";
    /// win32 app.rs fill_profiles (refilled on open), profile::menu
    MenuProfiles = "Lists the settings profiles; picking one starts PaTorrent in that profile in a window of its own, or brings it to the front.";
    /// win32 app.rs cmd::PROFILE_NEW -> new_profile -> profile::validate, start_profile
    MenuProfileNew = "Asks for a name, makes a new profile with its own settings and torrents, and starts PaTorrent in it.";
    /// win32 app.rs cmd::PROFILE_SHORTCUT -> profile_shortcut -> shortcut::create
    MenuProfileShortcut = "Puts a shortcut on the desktop that starts PaTorrent in this window's profile.";
    /// win32 app.rs cmd::PROFILE_FOLDER -> open_url(base_data_dir)
    MenuProfileFolder = "Opens the folder that holds this profile's settings and torrent data.";
    /// win32 app.rs cmd::CONNECT_REMOTE -> remote_ui::start_remote_window (patorrent.exe --remote)
    MenuConnectRemote = "Opens another window that connects to a patorrent-nox server on another computer and controls its torrents.";
    /// win32 app.rs cmd::EXIT -> quitting = true, DestroyWindow
    MenuExit = "Closes the window and quits PaTorrent, stopping all transfers, even when Close to Tray is on. In a remote window, the server's torrents keep running.";

    // ---- Menu: Options
    /// win32 app.rs cmd::PREFS -> show_preferences
    MenuPreferences = "Opens the Preferences window with the settings for connections, bandwidth, queueing, folders and the rest of the program.";
    /// win32 app.rs cmd::RSS -> show_rss
    MenuRss = "Opens the RSS Downloader, where you add feeds and set filters that download matching items automatically.";
    /// win32 app.rs cmd::SHOW_TOOLBAR -> toggles show_toolbar, update_visibility
    MenuShowToolbar = "Shows or hides the toolbar with its buttons and the search box.";
    /// win32 app.rs cmd::SHOW_DETAILS -> toggles show_details, update_visibility
    MenuShowDetails = "Shows or hides the tabs under the torrent list with the selected torrent's details, files, peers, trackers and speed.";
    /// win32 app.rs cmd::SHOW_STATUS -> toggles show_status, update_visibility
    MenuShowStatus = "Shows or hides the status bar at the bottom of the window with the transfer rates and totals.";
    /// win32 app.rs cmd::SHOW_TREE -> toggles show_tree, update_visibility
    MenuShowTree = "Shows or hides the category list on the left that filters the torrents by state and label.";
    /// win32 app.rs main_menu, AutoMode::MENU; frontend auto_shutdown
    MenuAutoShutdown = "Quits PaTorrent or puts the computer on standby, hibernates or shuts it down when the torrents are done, once something has run since you chose it. Lasts for this run only.";
    /// win32 app.rs cmd::AUTO_SHUTDOWN_BASE -> auto_shutdown.set_mode(None)
    ShutdownDisabled = "Turns Auto Shutdown off: nothing happens when the torrents are done.";
    /// win32 app.rs cmd::AUTO_SHUTDOWN_BASE -> set_mode(Quit, Downloads); frontend auto_shutdown::is_busy
    ShutdownQuitDownloads = "Quits PaTorrent after a 30 second countdown once no started torrent is downloading any more.";
    /// win32 app.rs cmd::AUTO_SHUTDOWN_BASE -> set_mode(Quit, Everything); frontend auto_shutdown::is_busy
    ShutdownQuitEverything = "Quits PaTorrent after a 30 second countdown once no started torrent is downloading or seeding any more.";
    /// win32 app.rs cmd::AUTO_SHUTDOWN_BASE -> set_mode(Standby, Downloads); frontend auto_shutdown::is_busy
    ShutdownStandbyDownloads = "Puts the computer on standby after a 30 second countdown once no started torrent is downloading any more.";
    /// win32 app.rs cmd::AUTO_SHUTDOWN_BASE -> set_mode(Standby, Everything); frontend auto_shutdown::is_busy
    ShutdownStandbyEverything = "Puts the computer on standby after a 30 second countdown once no started torrent is downloading or seeding any more.";
    /// win32 app.rs cmd::AUTO_SHUTDOWN_BASE -> set_mode(Hibernate, Downloads); frontend auto_shutdown::is_busy
    ShutdownHibernateDownloads = "Hibernates the computer after a 30 second countdown once no started torrent is downloading any more.";
    /// win32 app.rs cmd::AUTO_SHUTDOWN_BASE -> set_mode(Hibernate, Everything); frontend auto_shutdown::is_busy
    ShutdownHibernateEverything = "Hibernates the computer after a 30 second countdown once no started torrent is downloading or seeding any more.";
    /// win32 app.rs cmd::AUTO_SHUTDOWN_BASE -> set_mode(ShutDown, Downloads); frontend auto_shutdown::is_busy
    ShutdownShutDownDownloads = "Shuts down the computer after a 30 second countdown once no started torrent is downloading any more.";
    /// win32 app.rs cmd::AUTO_SHUTDOWN_BASE -> set_mode(ShutDown, Everything); frontend auto_shutdown::is_busy
    ShutdownShutDownEverything = "Shuts down the computer after a 30 second countdown once no started torrent is downloading or seeding any more.";
    /// win32 app.rs cmd::CLOSE_TO_TRAY -> toggles pref ui.close_to_tray (WM_CLOSE hides to the tray)
    MenuCloseToTray = "When on, closing the window hides PaTorrent to its tray icon and the torrents keep running. Exit still quits.";
    /// win32 app.rs cmd::MIN_TO_TRAY -> toggles pref ui.minimize_to_tray
    MenuMinimizeToTray = "When on, minimizing the window hides it from the taskbar, leaving only the tray icon.";
    /// win32 app.rs cmd::SETUP_GUIDE -> show_setup_guide -> setup_guide::show; frontend setup_guide.rs
    MenuSetupGuide = "Asks again the few questions of the first start: the download folder, the router, smart disk I/O and, where offered, starting with the system and the file associations.";

    // ---- Menu: Help
    /// win32 app.rs cmd::WEBSITE -> open_url(update::WEBSITE_URL)
    MenuWebsite = "Opens the PaTorrent website in your web browser.";
    /// win32 app.rs cmd::CHECK_UPDATES -> updates.start_manual, start_update_check
    MenuCheckUpdates = "Asks the PaTorrent website whether a newer version is out and tells you the answer.";
    /// win32 app.rs cmd::ABOUT -> dialogs::about(AboutText)
    MenuAbout = "Shows PaTorrent's version, copyright and licence, and where its settings file is.";
    /// win32 app.rs cmd::NOTICES -> notices::show
    MenuNotices = "Shows the licence texts of the third-party code PaTorrent is built on.";

    // ---- Menu: torrent list
    /// win32 app.rs cmd::OPEN_FOLDER -> torrent_action -> open_folder_target, show_in_explorer (also the Files tab's FILE_OPEN_FOLDER -> open_file)
    MenuOpenFolder = "Opens the folder that holds the selected item in Explorer, with the item selected.";
    /// win32 app.rs cmd::OPEN_FOLDER -> torrent_action -> open_folder_target, show_in_explorer
    MenuOpenFolderMany = "Opens the first selected torrent's download folder in Explorer, with its data selected once it exists.";
    /// win32 app.rs cmd::FORCE_START -> torrent_action -> actions::force_start
    MenuForceStart = "Starts the selected torrents now, outside the queue: they run even when the queue's limits on active torrents are reached.";
    /// win32 app.rs cmd::START -> torrent_action -> actions::start
    MenuStart = "Hands the selected torrents to the queue, which runs them as the limits on active torrents allow.";
    /// win32 app.rs cmd::PAUSE -> torrent_action -> actions::stop, set_paused(true)
    MenuPause = "Halts the selected torrents and takes them out of the queue. They show as Paused, also after a restart, until started again.";
    /// win32 app.rs cmd::STOP -> torrent_action -> actions::stop, set_paused(false)
    MenuStop = "Stops the selected torrents and takes them out of the queue. They show as Stopped, or Finished when done, until started again.";
    /// win32 app.rs cmd::UP -> torrent_action -> queue_position(QueueMove::Up)
    MenuQueueUp = "Moves the selected torrents one place up the queue, so they get a turn to run sooner.";
    /// win32 app.rs cmd::DOWN -> torrent_action -> queue_position(QueueMove::Down)
    MenuQueueDown = "Moves the selected torrents one place down the queue, so they get a turn to run later.";
    /// win32 app.rs cmd::QUEUE_TOP -> torrent_action -> queue_position(QueueMove::Top)
    MenuQueueTop = "Moves the selected torrents to the top of the queue, keeping their order, so they run first.";
    /// win32 app.rs cmd::QUEUE_BOTTOM -> torrent_action -> queue_position(QueueMove::Bottom)
    MenuQueueBottom = "Moves the selected torrents to the bottom of the queue, keeping their order, so they run last.";
    /// win32 app.rs cmd::REMOVE -> remove_selected(false) -> RemoveFlags::DELETE_PARTFILE
    MenuRemove = "Removes the selected torrents from the list, asking first when Preferences says so. The downloaded files stay on disk; only the part file holding pieces of skipped files is deleted.";
    /// win32 app.rs torrent menu -> submenu(menu, remove_and, MenuRemoveAnd)
    MenuRemoveAnd = "Removes the selected torrents from the list and also deletes their downloaded files.";
    /// win32 app.rs cmd::REMOVE_DATA -> remove_selected(true) -> RemoveFlags::DELETE_FILES
    MenuDeleteData = "Removes the selected torrents and deletes their downloaded files. The files do not go to the Recycle Bin.";
    /// win32 app.rs cmd::RECHECK -> torrent_action -> force_recheck
    MenuRecheck = "Checks the selected torrents' files on disk piece by piece and counts only the pieces that are complete and correct.";
    /// win32 app.rs cmd::REANNOUNCE -> torrent_action -> force_reannounce
    MenuUpdateTracker = "Contacts the selected torrents' trackers now, without waiting for the next announce, to report progress and get more peers.";
    /// win32 app.rs torrent menu -> submenu_icon(menu, adv, MenuAdvanced)
    MenuAdvanced = "Moves the selected torrents' files to another folder, or turns Initial Seeding on or off.";
    /// win32 app.rs cmd::SET_LOCATION -> set_download_location -> move_storage(AlwaysReplaceFiles)
    MenuSetLocation = "Picks a folder and moves the selected torrents' files there. Files of the same name already there are replaced.";
    /// win32 app.rs cmd::SUPER_SEED -> toggles TorrentFlags::SUPER_SEEDING (only when every selected torrent seeds)
    MenuInitialSeeding = "Turns initial seeding on or off for the selected seeds: each peer gets pieces no one else has yet, to spread a new torrent faster.";
    /// win32 app.rs torrent menu -> submenu_icon(menu, bw, MenuBandwidth)
    MenuBandwidth = "Sets a download or upload rate limit of its own for the selected torrents.";
    /// win32 app.rs cmd::DL_LIMIT_BASE -> set_download_limit (actions::LIMITS)
    MenuSetDownLimit = "Limits how fast each selected torrent downloads, or lifts its own limit. The global limits in Preferences still apply.";
    /// win32 app.rs cmd::UL_LIMIT_BASE -> set_upload_limit (actions::LIMITS)
    MenuSetUpLimit = "Limits how fast each selected torrent uploads, or lifts its own limit. The global limits in Preferences still apply.";
    /// win32 app.rs torrent menu -> submenu_icon(menu, lm, MenuLabel); set_label
    MenuLabel = "Gives the selected torrents a label, which groups them in the category list on the left.";
    /// win32 app.rs cmd::LABEL_NEW -> dialogs::input, set_label
    MenuNewLabel = "Asks for a new label name and gives it to the selected torrents.";
    /// win32 app.rs cmd::LABEL_NONE -> set_label(empty)
    MenuRemoveLabel = "Takes the label off the selected torrents.";
    /// win32 app.rs cmd::PROPERTIES -> show_properties -> properties::show, Changes::apply, Changes::store
    MenuProperties = "Opens the Torrent Properties of the selected torrents: trackers, bandwidth, seeding goal and other settings.";

    // ---- Menu: Files tab
    /// win32 app.rs cmd::FILE_OPEN -> open_file(folder = false) -> ShellExecuteW default verb
    MenuOpenFile = "Opens the selected file in its default program, as a double click in Explorer would. A file not on disk yet does nothing.";
    /// win32 app.rs cmd::FILE_BY_ORDER -> detail::by_file_order, set_file_priority
    MenuPrioritizeByOrder = "Gives the selected files falling priorities in their order in the torrent, so they finish one after another; skipped ones are downloaded again. With the filter on, it orders all the torrent's files.";
    /// win32 app.rs cmd::FILE_RELOCATE -> relocate_file -> dialogs::relocate_file, rename_file
    MenuRelocate = "Picks a new place for the selected file and moves it there, or, when it is not on disk, uses the file found there; a Force Re-check counts its pieces.";

    // ---- Menu: tray
    /// win32 app.rs cmd::TRAY_TOGGLE -> toggle_window (SW_HIDE)
    MenuHide = "Hides the PaTorrent window; the torrents keep running and the tray icon brings the window back.";
    /// win32 app.rs cmd::TRAY_TOGGLE -> toggle_window -> show_window
    MenuShow = "Brings the PaTorrent window back on screen and to the front.";
    /// win32 app.rs cmd::START_ALL -> actions::start on every torrent, set_paused(false)
    MenuStartAll = "Hands every torrent in the list to the queue, which runs them as the limits on active torrents allow.";
    /// win32 app.rs cmd::STOP_ALL -> actions::stop on every torrent, set_paused(false)
    MenuStopAll = "Stops every torrent in the list and takes them out of the queue until they are started again.";

    // ---- Toolbar
    /// win32 app.rs TOOLBAR cmd::ADD -> dialogs::open_torrents, add_sources
    ToolAddTorrent = "Picks one or more .torrent files and adds them, through the Add New Torrent dialog when Preferences has it turned on.";
    /// win32 app.rs TOOLBAR cmd::ADD_URL -> dialogs::input, add_sources
    ToolAddUrl = "Asks for a magnet link or an http or https address of a .torrent file and adds it. A link on the clipboard is filled in.";
    /// win32 app.rs TOOLBAR cmd::CREATE -> create_torrent
    ToolCreate = "Makes a new .torrent file from a file or folder on this computer, and can start seeding it at once.";
    /// win32 app.rs TOOLBAR cmd::REMOVE -> remove_selected(false)
    ToolRemove = "Removes the selected torrents from the list, asking first when Preferences says so. The downloaded files stay on disk; only the part file holding pieces of skipped files is deleted.";
    /// win32 app.rs TOOLBAR cmd::START -> torrent_action -> actions::start
    ToolStart = "Hands the selected torrents to the queue, which runs them as the limits on active torrents allow.";
    /// win32 app.rs TOOLBAR cmd::PAUSE -> torrent_action -> actions::stop, set_paused(true)
    ToolPause = "Halts the selected torrents and takes them out of the queue. They show as Paused until started again.";
    /// win32 app.rs TOOLBAR cmd::STOP -> torrent_action -> actions::stop, set_paused(false)
    ToolStop = "Stops the selected torrents and takes them out of the queue until they are started again.";
    /// win32 app.rs TOOLBAR cmd::UP -> torrent_action -> queue_position(QueueMove::Up)
    ToolQueueUp = "Moves the selected torrents one place up the queue, so they get a turn to run sooner.";
    /// win32 app.rs TOOLBAR cmd::DOWN -> torrent_action -> queue_position(QueueMove::Down)
    ToolQueueDown = "Moves the selected torrents one place down the queue, so they get a turn to run later.";
    /// win32 app.rs TOOLBAR cmd::RSS -> show_rss
    ToolRss = "Opens the RSS Downloader, where you add feeds and set filters that download matching items automatically.";
    /// win32 app.rs TOOLBAR cmd::PREFS -> show_preferences
    ToolPreferences = "Opens the Preferences window with the settings for connections, bandwidth, queueing, folders and the rest of the program.";
}
