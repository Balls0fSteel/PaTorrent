//! The English string table: the key of every user-visible string with its English text, the
//! source of every translation. Keys are grouped by where they show; `///` lines are notes for
//! translators and go into `lang/en.lng` as comments.
//!
//! Add new strings at the end of their group. A key is never reused for a different meaning:
//! rename it, so translations of the old text drop out.

macro_rules! strings {
    ($( $(#[doc = $note:literal])* $key:ident = $text:literal; )*) => {
        /// A string's key; [`crate::tr!`] gives its text in the current language.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum S { $($key),* }

        impl S {
            /// Every key, in table order.
            pub const ALL: &'static [S] = &[$(S::$key),*];
        }

        pub(super) const KEYS: &[&str] = &[$(stringify!($key)),*];
        pub(super) const ENGLISH: &[&str] = &[$($text),*];
        #[cfg(test)]
        pub(super) const NOTES: &[&str] = &[$(concat!($($note, "\n"),*)),*];
    };
}

strings! {
    // ---- The torrent list's Status column

    /// {error} is the engine's error message, in English.
    StatusError = "Error: {error}";
    /// Moving the torrent's files to another folder.
    StatusMoving = "Moving";
    StatusQueuedForChecking = "Queued for checking";
    /// {percent} is like "45.3 %".
    StatusChecking = "Checking {percent}";
    StatusQueuedSeed = "Queued Seed";
    StatusQueued = "Queued";
    StatusPaused = "Paused";
    StatusFinished = "Finished";
    StatusStopped = "Stopped";
    StatusDownloadingMetadata = "Downloading metadata";
    StatusSeeding = "Seeding";
    StatusDownloading = "Downloading";
    /// [S] marks initial seeding (super-seeding); keep the [S].
    StatusSuperSeeding = "[S] Seeding";
    /// [F] marks a torrent started with Force Start, outside the queue; keep the [F]. {status}
    /// is one of the statuses above.
    StatusForced = "[F] {status}";
    /// Preferences > Connection > "Use only this network" finds no matching network adapter
    /// (the VPN is down): the torrent resumes by itself when it comes back.
    StatusWaitingNetwork = "Waiting for the network";

    // ---- The category list

    CatTorrents = "Torrents";
    CatDownloading = "Downloading";
    CatCompleted = "Completed";
    CatActive = "Active";
    CatInactive = "Inactive";

    // ---- Column titles: the torrent list and the detail tabs' tables

    ColName = "Name";
    /// The queue position column; a short symbol.
    ColQueue = "#";
    ColSize = "Size";
    ColDone = "Done";
    ColStatus = "Status";
    ColSeeds = "Seeds";
    ColPeers = "Peers";
    ColDownSpeed = "Down Speed";
    ColUpSpeed = "Up Speed";
    /// Estimated time left.
    ColEta = "ETA";
    ColUploaded = "Uploaded";
    ColDownloaded = "Downloaded";
    ColRatio = "Ratio";
    /// Availability, abbreviated.
    ColAvailability = "Avail.";
    ColLabel = "Label";
    ColAddedOn = "Added On";
    ColCompletedOn = "Completed On";
    ColTracker = "Tracker";
    ColDownLimit = "Down Limit";
    ColUpLimit = "Up Limit";
    ColRemaining = "Remaining";
    ColSeedsPeers = "Seeds/Peers";
    /// Where the torrent's files are saved.
    ColSaveAs = "Save As";
    /// Bytes thrown away (failed hash checks, duplicates).
    ColWasted = "Wasted";
    /// Trackers tab: time until the next announce.
    ColUpdateIn = "Update In";
    ColIp = "IP";
    /// Peers tab: the peer's BitTorrent program.
    ColClient = "Client";
    ColFlags = "Flags";
    ColPercent = "%";
    /// Peers tab: requests queued in each direction.
    ColReqs = "Reqs";
    /// Pieces tab: the piece number.
    ColPiece = "#";
    ColBlockCount = "# of Blocks";
    ColBlocks = "Blocks";
    /// Pieces tab: blocks of the piece completed.
    ColBlocksCompleted = "Completed";
    ColPieceCount = "# Pieces";
    ColFirstPiece = "First Piece";
    ColPriority = "Priority";
    /// RSS: the episode number of a feed item.
    ColEpisode = "Episode";
    /// RSS: the release's qualities (HDTV, 720p...).
    ColFormat = "Format";
    ColDate = "Date";
    ColFeed = "Feed";
    ColUrl = "URL";

    // ---- Trackers tab: a tracker's status

    TrackerWorking = "Working";
    /// {message} is the tracker's own message.
    TrackerWorkingMessage = "Working: {message}";
    Updating = "Updating...";
    TrackerError = "Error";

    // ---- File priorities

    PrioDontDownload = "Don't Download";
    PrioLow = "Low";
    PrioNormal = "Normal";
    PrioHigh = "High";
    PrioHighItem = "High Priority";
    PrioNormalItem = "Normal Priority";
    PrioLowItem = "Low Priority";

    // ---- Units and times (sizes, speeds, ETAs)

    /// Bytes.
    UnitB = "B";
    /// Kibibytes, written kB as µTorrent did.
    UnitKb = "kB";
    UnitMb = "MB";
    UnitGb = "GB";
    UnitTb = "TB";
    UnitPb = "PB";
    UnitKbPerSec = "kB/s";
    UnitMbPerSec = "MB/s";
    /// Short durations: s seconds, m minutes, h hours, d days, w weeks.
    DurSeconds = "{s}s";
    DurMinutes = "{m}m {s}s";
    DurHours = "{h}h {m}m";
    DurDays = "{d}d {h}h";
    DurWeeks = "{w}w {d}d";

    // ---- Menus (& marks the mnemonic)

    MenuFile = "&File";
    MenuOptions = "&Options";
    MenuHelp = "&Help";
    MenuAddTorrent = "&Add Torrent...";
    MenuAddUrl = "Add Torrent from &URL...";
    MenuCreate = "&Create New Torrent...";
    /// Connect this window to a rookparcel-nox on another computer.
    MenuConnectRemote = "Connect to &Remote...";
    /// File > Profiles (D42): the profiles, each started in a window of its own.
    MenuProfiles = "Pro&files";
    /// The default profile's item in File > Profiles (the named ones show their names).
    MenuProfileDefault = "Default";
    MenuProfileNew = "&New Profile...";
    /// A desktop shortcut (Windows) or an applications menu item (Linux) for this profile.
    MenuProfileShortcut = "Create &Shortcut...";
    MenuProfileFolder = "&Open Profile Folder";
    ProfileNewTitle = "New Profile";
    ProfileNewPrompt = "Name (letters a-z, digits and hyphens):";
    ProfileShortcutDesktop = "The shortcut is on your desktop:\n{path}";
    ProfileShortcutMenu = "{name} is in your applications menu now.";
    MenuExit = "E&xit";
    MenuPreferences = "&Preferences";
    MenuRss = "&RSS Downloader...";
    MenuShowToolbar = "Show &Toolbar";
    MenuShowDetails = "Show &Detailed Info";
    MenuShowStatus = "Show &Status Bar";
    MenuShowTree = "Show &Category List";
    MenuAutoShutdown = "&Auto Shutdown";
    MenuCloseToTray = "Close to Tra&y";
    MenuMinimizeToTray = "&Minimize to Tray";
    MenuAbout = "&About Rookparcel";
    /// Help menu: opens a window with the licence texts of the code Rookparcel is built on
    /// (THIRD-PARTY-NOTICES.md); without the mnemonic it is also the window's title.
    MenuNotices = "Third-Party &Notices";
    /// Help menu: asks the project website whether a newer Rookparcel is out; without the &
    /// it is also the title of the message box that answers.
    MenuCheckUpdates = "Check for &Updates";
    /// Help menu: opens the project website in the browser.
    MenuWebsite = "Rookparcel &Website";
    /// The Ctrl key, as menus write shortcuts (Ctrl+O).
    KeyCtrl = "Ctrl";
    /// The Shift key, as menus write shortcuts (Shift+Del).
    KeyShift = "Shift";
    /// The Delete key, as menus write shortcuts (Del).
    KeyDel = "Del";
    /// The Enter key, as menus write shortcuts (Enter).
    KeyEnter = "Enter";
    MenuHide = "&Hide Rookparcel";
    MenuShow = "&Show Rookparcel";
    MenuStartAll = "S&tart All Torrents";
    MenuStopAll = "St&op All Torrents";
    /// Shown when one finished torrent is selected.
    MenuOpenFolder = "Open &Containing Folder";
    MenuOpenFolderMany = "Open Containing &Folder";
    /// The Files tab's menu: opens the file in its default program.
    MenuOpenFile = "&Open";
    /// The Files tab's menu: the selected files download in their order in the torrent.
    MenuPrioritizeByOrder = "Prioritize by File Order";
    /// The Files tab's menu: moves the file, or points it at where it is now.
    MenuRelocate = "Relocate...";
    PickRelocate = "Select the file's new location";
    MenuForceStart = "&Force Start";
    MenuStart = "&Start";
    MenuPause = "&Pause";
    MenuStop = "S&top";
    MenuQueueUp = "Move &Up Queue";
    MenuQueueDown = "Move &Down Queue";
    MenuQueueTop = "Move to &Top of Queue";
    MenuQueueBottom = "Move to &Bottom of Queue";
    MenuRemove = "&Remove";
    /// "Remove And" > "Delete Data": remove the torrent and delete its files.
    MenuRemoveAnd = "Remove &And";
    MenuDeleteData = "Delete &Data";
    MenuRecheck = "Force Re-&Check";
    MenuUpdateTracker = "&Update Tracker";
    MenuAdvanced = "Ad&vanced";
    MenuSetLocation = "Set &Download Location...";
    MenuInitialSeeding = "&Initial Seeding";
    MenuBandwidth = "&Bandwidth Allocation";
    MenuSetDownLimit = "Set &Download Limit";
    MenuSetUpLimit = "Set &Upload Limit";
    MenuUnlimited = "Unlimited";
    MenuLabel = "&Label";
    MenuNewLabel = "&New Label...";
    MenuRemoveLabel = "&Remove Label";
    MenuProperties = "Pr&operties";
    MenuDownload = "&Download";
    MenuAddFeed = "&Add RSS Feed...";
    MenuEditFeed = "&Edit Feed...";
    MenuDisableFeed = "&Disable Feed";
    MenuEnableFeed = "E&nable Feed";
    MenuUpdateFeed = "&Update Feed";
    MenuDeleteFeed = "De&lete Feed";
    MenuCheck = "Check";
    MenuUncheck = "Uncheck";
    MenuSelectAll = "Select &All";
    MenuSelectNone = "Select N&one";
    /// Options menu (D53): opens the Setup Guide, the few questions asked at a profile's first
    /// start; µTorrent 2.2.1's item.
    MenuSetupGuide = "Setup &Guide";

    // ---- Toolbar buttons (tooltips)

    ToolAddTorrent = "Add Torrent";
    ToolAddUrl = "Add Torrent from URL";
    ToolCreate = "Create New Torrent";
    ToolRemove = "Remove";
    ToolStart = "Start";
    ToolPause = "Pause";
    ToolStop = "Stop";
    ToolQueueUp = "Move Up Queue";
    ToolQueueDown = "Move Down Queue";
    ToolRss = "RSS Downloader";
    ToolPreferences = "Preferences";

    // ---- The category list, more

    CatLabels = "Labels";
    CatNoLabel = "No Label";
    CatFeeds = "Feeds";

    // ---- Detail tabs and the General tab

    TabGeneral = "General";
    TabTrackers = "Trackers";
    TabPeers = "Peers";
    TabPieces = "Pieces";
    TabFiles = "Files";
    TabSpeed = "Speed";
    TabLogger = "Logger";
    /// Logger tab: the column of log lines.
    ColMessage = "Message";
    GenSelectTorrent = "Select a torrent to see its details.";
    GenDownloaded = "Downloaded:";
    GenAvailability = "Availability:";
    /// Section heading.
    GenTransfer = "Transfer";
    /// Section heading.
    GenGeneral = "General";
    GenTimeElapsed = "Time Elapsed:";
    GenRemaining = "Remaining:";
    GenWasted = "Wasted:";
    GenUploaded = "Uploaded:";
    GenSeeds = "Seeds:";
    GenPeers = "Peers:";
    /// {count} connected of {total} in the swarm.
    GenConnected = "{count} of {total} connected";
    GenDownloadSpeed = "Download Speed:";
    GenUploadSpeed = "Upload Speed:";
    GenDownLimit = "Down Limit:";
    GenUpLimit = "Up Limit:";
    GenShareRatio = "Share Ratio:";
    GenStatus = "Status:";
    GenSaveAs = "Save As:";
    GenTotalSize = "Total Size:";
    GenAddedOn = "Added On:";
    GenPieces = "Pieces:";
    GenCompletedOn = "Completed On:";
    GenCreatedOn = "Created On:";
    GenCreatedBy = "Created By:";
    GenHash = "Hash:";
    GenTrackers = "Trackers:";
    GenComment = "Comment:";
    /// {count} pieces of {size} each, {have} of them downloaded.
    GenPiecesValue = "{count} x {size} (have {have})";
    PiecesWaiting = "Waiting for piece information...";
    PiecesSelectTorrent = "Select a torrent.";
    /// The Pieces tab's summary: {count} pieces, {done} downloaded.
    PiecesSummary = "{count} pieces, {done} done. Darker squares are pieces more peers have.";
    /// The same with {downloading} pieces being downloaded, drawn orange.
    PiecesSummaryDownloading = "{count} pieces, {done} done, {downloading} downloading (orange). Darker squares are pieces more peers have.";
    /// Below the piece map when it does not fit: {count} pieces not drawn.
    PiecesMore = "... {count} more";
    /// Speed tab legend; {rate} is like "12.0 kB/s".
    SpeedDownload = "Download: {rate}";
    SpeedUpload = "Upload: {rate}";
    /// General tab: the public address trackers report back to us (an IPv4 and an IPv6
    /// address, or NetExternalUnknown), so the user can see whether a VPN is in use.
    GenExternal = "Your address as trackers see it:";

    // ---- The status bar and the tray

    NetOk = "Network OK";
    NetNoIncoming = "No incoming connections";
    /// The listen port is held by another program (often another Rookparcel profile); a click
    /// opens Preferences > Connection.
    NetPortInUse = "Port {port} in use";
    BarDht = "DHT: {count} nodes";
    /// D = download, T = total.
    BarDown = "D: {rate} T: {total}";
    /// U = upload, T = total.
    BarUp = "U: {rate} T: {total}";
    /// The tray icon's tooltip; D = download, U = upload; {version} is Rookparcel's (1.0.0).
    TrayTip = "Rookparcel {version}\nD: {down} U: {up}";
    BalloonComplete = "Download Complete";
    /// The window title of a window connected to a rookparcel-nox; {label} is host:port.
    TitleRemote = "Rookparcel {version} - {label} (remote)";
    TitleDisconnected = "Rookparcel {version} - {label} (disconnected)";
    /// The main window's title with "Show current speed in the title bar" on: D = download,
    /// U = upload; {title} is the normal title (Rookparcel 1.0.0).
    TitleSpeed = "D:{down} U:{up} - {title}";
    /// BarDown and BarUp while Keep ping low limits the speed: {cap} is the automatic limit,
    /// a speed like 450 kB/s.
    BarDownAuto = "D: {rate} (auto {cap}) T: {total}";
    BarUpAuto = "U: {rate} (auto {cap}) T: {total}";
    /// The status bar's network part while "Use only this network" finds no matching network
    /// adapter (the VPN is down); torrents wait until it comes back.
    NetWaiting = "Waiting for the network";
    /// Before the network part of the status bar at start, until the saved torrents are back in
    /// the list (a settings folder on a slow drive can take a while).
    BarRestoring = "Restoring torrents...";
    /// The status bar's network tooltip: {address} is one or two IP addresses, or
    /// NetExternalUnknown.
    NetExternal = "Your address as trackers see it: {address}";
    /// No tracker has told us our public address yet.
    NetExternalUnknown = "not known yet";

    // ---- Logger lines the frontends write ({name} is a torrent's name)

    LogAdded = "{name}: added";
    LogFinished = "{name}: finished downloading";
    LogMovingCompleted = "{name}: moving the completed download to {dir}";
    LogMovingTo = "{name}: moving to {dir}";
    /// {mode} is a scheduler mode (Full speed, Limited...).
    LogScheduler = "Scheduler: {mode}";
    LogShutdownNewWork = "Auto Shutdown: new work, the countdown is off until it is done";
    /// {what} is one of the four below; {secs} seconds.
    LogShutdownDone = "Auto Shutdown: done, {what} in {secs} s";
    LogShutdownCancelled = "Auto Shutdown: cancelled, now Disabled";
    /// {what} is one of the four below, or a menu item, or an error.
    LogShutdown = "Auto Shutdown: {what}";
    ShutdownWhatQuit = "quitting";
    ShutdownWhatStandby = "standby";
    ShutdownWhatHibernate = "hibernating";
    ShutdownWhatShutDown = "shutting down";
    LogPrefsNotSaved = "could not save the preferences: {error}";
    /// {file} is a path.
    LogNotSaved = "could not save {file}: {error}";
    LogAutoloadError = "Automatically load .torrents: {error}";
    /// {line} is a .torrent folder Logger line.
    LogAlreadyInList = "{line} (already in the list)";
    LogIpFilterOff = "IP filter off";
    LogIpFilterRules = "IP filter: {count} rules from {path}";
    LogIpFilterError = "IP filter: {path}: {error}";
    /// The same, when the rules loaded before stay in force.
    LogIpFilterKept = "IP filter: {path}: {error} (the old rules stay)";
    /// {title} is an RSS item's title.
    LogRssAlreadyInList = "RSS: {title}: already in the torrent list";
    /// {version} is a newer Rookparcel's version (1.0.1), {url} its download page.
    LogUpdateAvailable = "Rookparcel {version} is available: {url}";
    LogUpdateFailed = "Update check failed: {error}";
    /// {what} is one of the two below.
    LogSeedingGoal = "{name}: seeding goal reached, {what}";
    SeedingGoalStopped = "stopped";
    SeedingGoalLimited = "upload limited to {rate}";

    // ---- Message boxes and errors

    ErrEngineStart = "Cannot start the BitTorrent engine:\n{error}";
    AskAssociate = "Rookparcel is not the program that opens .torrent files and magnet: links.\n\nAssociate them with Rookparcel?\n\n(Preferences > General > \"Check association on startup\" turns this question off.)";
    /// {note} says which program Windows still uses.
    AskDefaultApps = "{note}\n\nWindows keeps a program chosen in Windows itself, and only you can change that choice, in Settings > Default apps. Open Default apps now?";
    ErrAssociate = "Could not associate Rookparcel:\n{error}";
    ErrIntegration = "Could not change the Windows integration:\n{error}";
    ErrDesktopIntegration = "Could not change the desktop integration:\n{error}";
    AssocTorrent = "Windows still opens .torrent files with {program}.";
    AssocMagnet = "Windows still opens magnet: links with {program}.";
    AssocBoth = "Windows still opens .torrent files and magnet: links with {program}.";
    AssocEach = "Windows still opens .torrent files with {torrent}, magnet: links with {magnet}.";
    /// {note} is one of the four above.
    AssocChoose = "{note} Choose Rookparcel in Default apps.";
    /// Another profile of this Rookparcel owns the associations (D42); `{profile}` is its name
    /// (`default` for the default profile).
    AssocOtherProfile = "The Rookparcel profile \"{profile}\" opens them now; ticking a box makes this profile open them instead.";
    /// Windows' name for the .torrent file type (Explorer's Type column).
    AssocTorrentType = "Torrent File";
    /// Windows' name for magnet: links.
    AssocMagnetType = "Magnet link";
    /// Rookparcel's description in Windows' Default apps.
    AppDescription = "BitTorrent client";
    ErrAddTorrent = "Could not add {name}:\n{error}";
    /// {label} is host:port, {message} what went wrong.
    ErrRemoteLost = "The connection to {label} was lost:\n{message}\n\nThe list shows the torrents as they were. Close this window and use File > Connect to Remote... to connect again.";
    ErrMoveFiles = "Could not move the files of {name}:\n{error}";
    /// {url} is a web address.
    ErrDownloadUrl = "Could not download {url}:\n{error}";
    /// {path} is a file.
    ErrOpenFile = "Could not open {path}:\n{error}";
    InfoAlreadyInList = "\"{name}\" is already in the torrent list.";
    AskRemove = "Remove \"{name}\" from the list?";
    AskRemoveMany = "Remove the selected torrents ({count}) from the list?";
    AskRemoveData = "Remove \"{name}\" and delete the downloaded data?";
    AskRemoveDataMany = "Remove the selected torrents ({count}) and delete the downloaded data?";
    /// Under the questions above.
    RemoveKeepsData = "Downloaded data stays on disk.";
    /// Windows: deleted files skip the Recycle Bin.
    RemoveDeletesData = "The files cannot be recovered from the Recycle Bin.";
    /// Linux, BSD and the Web UI.
    RemoveDeletesDataTrash = "The files are deleted from the disk, not moved to the trash.";
    BtnRemove = "Remove";
    /// Message headings; the error follows on its own.
    CannotStartEngine = "Cannot start the BitTorrent engine";
    CannotAdd = "Could not add {name}";
    CannotOpen = "Could not open {path}";
    CannotDownload = "Could not download {url}";
    /// The Files tab's Relocate (or a rename) refused, for example by a file already there.
    CannotRelocate = "Could not move or rename a file of {name}";
    ErrNotALink = "Not a link Rookparcel can add";
    /// Keep the example links as they are.
    UrlPlaceholder = "magnet:?xt=… or https://…/file.torrent";
    /// Remove the torrent and delete its files (one menu item).
    MenuRemoveAndDelete = "Remove &And Delete Data";
    AboutShort = "A BitTorrent client in the spirit of µTorrent 2.2.1, on a Rust port of libtorrent.";
    AskDeleteFeed = "Delete the feed \"{name}\"?";
    ErrNotRunning = "not running";
    /// In the status bar, until clicked, when a newer Rookparcel is out ({version}, like 1.0.1);
    /// clicking it opens the download page.
    UpdateNotice = "Rookparcel {version} is available - click here to download it";
    /// Help > Check for Updates; {version} is the newer one, {current} the running one.
    UpdateNewer = "Rookparcel {version} is available. You have {current}.\n\nOpen its download page?";
    UpdateNewest = "You have the newest version, Rookparcel {version}.";
    UpdateFailed = "Could not check for updates: {error}";
    /// The same question on Windows for an installed Rookparcel, which can update itself: the
    /// buttons are the two texts below and Cancel.
    UpdateInstallAsk = "Rookparcel {version} is available. You have {current}.\n\nDownload and install it now?";
    UpdateInstallBtn = "Download and install";
    UpdateOpenPageBtn = "Open download page";
    /// The title of the small window with the progress bar while the setup downloads.
    UpdateDownloading = "Downloading Rookparcel {version}...";
    /// {error} is the download's or the checksum's error, in English.
    UpdateDownloadFailed = "Could not download Rookparcel {version}: {error}";
    /// The setup is downloaded and checked; Rookparcel has to close for it to install.
    UpdateInstallNow = "Rookparcel {version} is downloaded. To install it, Rookparcel closes now and starts again when Setup is done.\n\nClose and install now?";
    /// {version} is like 1.0.0, {path} the settings file. Keep the names and licences as they are.
    AboutText = "Rookparcel {version}\n\nA re-creation of µTorrent 2.2.1, built on a Rust port of libtorrent-rasterbar 2.0.11.\n\nCopyright 2026 DukeCorp1996. Free for noncommercial use under the PolyForm Noncommercial License 1.0.0.\n\nPorted libtorrent code: Copyright (c) 2003-2020, Arvid Norberg and contributors, BSD-3-Clause. License texts: LICENSE.md, THIRD-PARTY-NOTICES.md, or run rookparcel-nox --license.\n\nNot affiliated with libtorrent, qBittorrent, µTorrent or BitTorrent, Inc.\n\nSettings: {path}";

    // ---- Small dialogs: Add Torrent from URL, New Label, folder pickers, file filters

    BtnOk = "OK";
    BtnCancel = "Cancel";
    BtnApply = "&Apply";
    BtnClose = "Close";
    AddUrlTitle = "Add Torrent from URL";
    AddUrlPrompt = "Please enter the location of the torrent you want to open (a URL or a magnet link):";
    NewLabelTitle = "New Label";
    NewLabelPrompt = "Enter the name of the new label:";
    PickLocation = "Select the new download location";
    /// File dialog filters.
    FilterTorrents = "Torrents (*.torrent)";
    FilterAllFiles = "All Files (*.*)";
    PickTorrent = "Select a .torrent to open";
    PickFile = "Select a file";

    // ---- Add New Torrent

    AddTitle = "Add New Torrent";
    AddSaveIn = "Save In";
    AddName = "&Name";
    AddOptions = "Torrent Options";
    AddSkipCheck = "Skip &hash check";
    AddStart = "&Start torrent";
    AddTopOfQueue = "Add to &top of queue";
    AddLabel = "&Label:";
    AddContents = "Torrent Contents";
    AddTorrentName = "Name:";
    AddComment = "Comment:";
    AddSize = "Size:";
    AddDate = "Date:";
    /// A folder whose files have different priorities.
    AddMixed = "Mixed";
    AddPickFolder = "Select the folder to save the torrent in";
    AddNoFolder = "Please choose the folder to save the torrent in.";
    /// After the size: free space on the save folder's drive.
    AddDiskSpace = "(disk space: {free} free)";
    /// The check box that turns Preferences' "Show a window that displays the files inside the torrent" off.
    AddDontShowAgain = "Don't show this again";
    /// The Name field's label on a line (the Win32 dialog; `AddName` titles a box in the others).
    AddNameLine = "&Name:";

    // ---- Create New Torrent

    CreateTitle = "Create New Torrent";
    CreateSource = "Select So&urce";
    CreateAddFile = "Add &file";
    CreateAddDir = "Add &directory";
    CreateSkip = "S&kip Files";
    CreateProps = "Torrent Properties";
    CreateTrackers = "&Trackers:";
    CreateWebSeeds = "&Web Seeds:";
    CreateComment = "C&omment:";
    CreatePieceSize = "&Piece size:";
    /// The BitTorrent version: v1, v2 or both.
    CreateVersion = "Versio&n:";
    CreateOther = "Other";
    CreateSeed = "&Start seeding";
    CreatePrivate = "P&rivate torrent";
    CreatePreserveOrder = "Preser&ve file order";
    CreateSave = "&Create and save as...";
    CreatePickDir = "Select a directory to create the torrent from";
    CreatePickSave = "Select where to save the torrent";
    CreateFailed = "Could not create the torrent:\n{error}";

    // ---- Torrent Properties

    /// {name} is a torrent's name.
    PropsTitle = "Properties - {name}";
    PropsTitleMany = "Properties - {count} torrents";
    PropsTrackers = "Trackers (a blank line between tiers)";
    PropsBandwidth = "Bandwidth Settings";
    PropsUpRate = "Maximum upload rate (kB/s): [0: unlimited]";
    PropsDownRate = "Maximum download rate (kB/s): [0: unlimited]";
    PropsSlots = "Number of upload slots: [0: unlimited]";
    PropsOverrideSlots = "Override default number of upload slots";
    PropsSeedingGoal = "Seeding Goal";
    PropsOverrideGoal = "Override default settings";
    PropsRatio = "Minimum ratio (%): [0: no ratio goal]";
    PropsSeedTime = "Minimum seeding time (minutes): [0: no time goal]";
    PropsOther = "Other Settings";
    PropsInitialSeeding = "Initial Seeding";
    PropsDht = "Enable DHT";
    PropsPex = "Peer Exchange";

    // ---- Connect to Remote

    RemoteTitle = "Connect to Remote";
    RemoteHost = "&Host:";
    RemotePort = "&Port:";
    RemoteUser = "&User name:";
    RemotePassword = "Pass&word:";
    RemoteHttps = "Use HTTP&S";
    RemoteNote = "The Web UI login of a rookparcel-nox started with --webui. Its torrents stay on that computer.";
    RemoteConnect = "Connect";
    RemoteBadAddress = "Enter the server's host name or address, and its port (1 to 65535).";
    /// {name} is host:port, {error} what went wrong.
    RemoteCertChanged = "The certificate of {name} is not the one you accepted before:\n{error}\n\nIf it was replaced on purpose (a new --webui-cert, or webui-cert.pem renewed), connect again to check the new one. Otherwise someone may be in between.";
    RemoteCannotConnect = "Cannot connect to {name}:\n{error}";
    RemoteHintNotNox = "Is it a rookparcel-nox with its Web UI on this port (--webui)?";
    RemoteHintHttps = "If the server's Web UI uses HTTPS (--webui-https), tick Use HTTPS.";
    RemoteNoHttps = "Cannot reach {name} over HTTPS:\n{error}";
    /// {fingerprint} is a long hexadecimal number.
    RemoteTrust = "{name} presents a certificate that this computer does not trust (a self-signed one, as rookparcel-nox --webui-https makes).\n\nSHA-256 fingerprint:\n{fingerprint}\n\nCompare it with the \"certificate SHA-256\" line rookparcel-nox printed when it started. Trust this certificate for {name}?";
    /// Asked before connecting without HTTPS to a host other than this computer.
    RemotePlaintext = "{name} is another computer, and without HTTPS your user name, password and every command cross the network unencrypted: anyone on it can read or change them.\n\nConnect without HTTPS anyway?";
    ErrStartRookparcel = "Cannot start Rookparcel: {error}";
    /// A second copy was started with a torrent while the first runs with administrator rights.
    ForwardRefused = "Rookparcel is already running as administrator, and Windows does not let this copy hand it what to add. Add it from the running Rookparcel, or start Rookparcel without administrator rights.";
    /// The folder's `rookparcel.lock` is held: another user session, or another computer that
    /// opened the same folder over a share (D42).
    FolderInUse = "Another Rookparcel is already using this folder, in another session or on another computer that shares it:\n\n{folder}\n\nClose that Rookparcel first.";
    /// macOS runs a quarantined Rookparcel.app from a temporary read-only copy (App
    /// Translocation), where a portable `rookparcel.ini` beside it is not seen (D42).
    MacTranslocated = "macOS is running Rookparcel from a temporary copy, so a rookparcel.ini beside Rookparcel.app is not seen. Move Rookparcel.app to the folder you want (or to Applications) with Finder and start it again.";
    /// Portable mode (a `rookparcel.ini` beside the program) in a folder that cannot be written
    /// (D42): Rookparcel does not fall back to the user's own folder.
    PortableReadOnly = "This portable Rookparcel cannot write to its folder:\n\n{folder}\n{error}\n\nMove the folder where you can write, or remove rookparcel.ini beside the program to keep the settings in your user folder.";
    /// `--profile` with a name that cannot be used (D42); `{name}` is what was given.
    ProfileBadName = "\"{name}\" cannot be a profile name. A profile name has 1 to 32 characters: letters a-z, digits and hyphens (-), starting with a letter.";
    /// Asked at the first start of a named profile, before its folder is made.
    ProfileCreate = "The profile \"{name}\" does not exist yet. Create it?\n\nIt starts empty, like a new installation, in:\n{folder}";
    /// Windows lists it beside Rookparcel while it holds up a sign-out or shutdown to save its
    /// state.
    EndSessionSaving = "Saving the torrent list and progress...";

    // ---- The window shown while Rookparcel closes (after a second, or at a Windows sign-out)

    ExitTitle = "Rookparcel is closing";
    /// {time} is minutes:seconds, like 1:05.
    ExitElapsed = "Closing for {time}";
    ExitStopping = "Stopping the torrents...";
    ExitMoving = "Waiting for files to finish moving. Closing in the middle could leave a torrent half in one folder and half in another, so Rookparcel waits.";
    /// The list of moves: a column title, then its cells. {done} and {total} are sizes like
    /// 1.2 GB; a move that has not started yet shows ExitMoveWaiting.
    ExitColMoveTo = "Moving to";
    ExitMoveProgress = "{done} of {total}";
    ExitMoveWaiting = "Waiting";
    ExitSaving = "Saving the torrent list and progress. This step always finishes, so that nothing already downloaded is lost.";
    ExitTrackers = "Telling the trackers that Rookparcel stopped...";
    /// {size} is like 3.2 MB.
    ExitWriting = "Writing the last downloaded data to disk and closing files ({size} left)...";
    /// The button; the same & rules as the menus.
    ExitQuitNow = "&Quit Now";
    /// Asked after Quit Now; {list} is one or more of the lines below, each starting with "• ".
    ExitAskQuit = "Quit now, without waiting?\n\n{list}\n\nThe torrent list and progress are saved first in any case.";
    /// {name} is the torrent, {folder} where its files stay, {dest} where they were going.
    /// "Set Download Location" is the menu item MenuSetLocation.
    ExitAbortMove = "• \"{name}\" stops moving. Its files stay in {folder}; what was already copied to {dest} is deleted. Move it again later with Set Download Location.";
    /// After two ExitAbortMove lines when more torrents are moving; {count} is 2 or more.
    ExitAbortMoreMoves = "• {count} more torrents stop moving in the same way.";
    ExitAbortTrackers = "• The trackers are not told that Rookparcel stopped; they forget it by themselves after a while.";
    /// {size} is like 3.2 MB.
    ExitAbortWrites = "• {size} that was downloaded but not yet written is downloaded again next time. Nothing already on disk is affected.";

    // ---- Auto Shutdown

    ShutdownTitle = "Auto Shutdown";
    ShutdownDownloadsDone = "All downloads have completed.";
    ShutdownEverythingDone = "All torrents have completed.";
    /// {time} is minutes:seconds, like 0:30.
    ShutdownWillQuit = "Rookparcel will quit in {time}.";
    ShutdownWillStandby = "The computer will go on standby in {time}.";
    ShutdownWillHibernate = "The computer will hibernate in {time}.";
    ShutdownWillShutDown = "The computer will shut down in {time}.";
    ShutdownErrStandby = "Could not put the computer on standby:\n{error}";
    ShutdownErrHibernate = "Could not hibernate the computer:\n{error}";
    ShutdownErrShutDown = "Could not shut down the computer:\n{error}";
    /// A test mode: {name} is an environment variable, {what} one of the three below.
    ShutdownDryRun = "{name} is set: did not {what}.";
    PowerStandby = "put the computer on standby";
    PowerHibernate = "hibernate the computer";
    PowerShutDown = "shut down the computer";

    // ---- Preferences: pages and controls

    PrefTitle = "Preferences";
    PageGeneral = "General";
    PageUi = "UI Settings";
    PageDirectories = "Directories";
    PageConnection = "Connection";
    PageBandwidth = "Bandwidth";
    PageBitTorrent = "BitTorrent";
    PageQueueing = "Queueing";
    PageScheduler = "Scheduler";
    PageWebUi = "Web UI";
    PageAdvanced = "Advanced";
    PageDiskCache = "Disk Cache";
    PrefLanguage = "Language";
    PrefUiLanguage = "User interface:";
    PrefIntegration = "Windows Integration";
    /// The GTK client's group on Preferences > General (Linux, the BSDs).
    PrefDesktopIntegration = "Desktop Integration";
    PrefAssocTorrent = "Associate with .torrent files";
    PrefAssocMagnet = "Associate with magnet: links";
    PrefCheckAssoc = "Check association on startup";
    PrefAutostart = "Start Rookparcel on system startup";
    PrefStartMinimized = "Start minimized";
    /// Under Desktop Integration when the GTK client runs as a Flatpak (the boxes are greyed).
    PrefDesktopPackaged = "Rookparcel runs as a Flatpak: the desktop offers it for .torrent files and magnet: links. Choose the default program in your desktop's settings.";
    PrefDefaultApps = "Default apps...";
    /// Preferences > General, as in µTorrent 2.2.1: the update check.
    PrefPrivacy = "Privacy";
    PrefCheckUpdates = "Check for updates automatically";
    /// Pre-releases (release candidates) are offered too.
    PrefUpdateBeta = "Update to beta versions";
    PrefDisplay = "Display Options";
    PrefConfirmDelete = "Confirm when deleting torrents";
    /// Every other row of the torrent list (and the other lists) gets a light grey background.
    PrefAlternateRows = "Alternate list background color";
    /// The main window's title starts with the download and upload speeds (TitleSpeed).
    PrefTitleSpeed = "Show current speed in the title bar";
    PrefBalloons = "Show balloon notifications in tray";
    PrefTray = "System Tray";
    PrefCloseToTray = "Close to tray";
    PrefMinimizeToTray = "Minimize to tray";
    PrefWhenAdding = "When Adding Torrents";
    PrefDontStart = "Don't start the download automatically";
    PrefActivateWindow = "Activate the program window";
    PrefShowAddDialog = "Show a window that displays the files inside the torrent";
    PrefDownloadLocation = "Location of Downloaded Files";
    PrefNewDownloadsIn = "Put new downloads in:";
    PrefMoveCompleted = "Move completed downloads to:";
    PrefMoveOnlyDefault = "Only move from the default download directory";
    /// Under "Move completed downloads to:": moves into a sub-folder named after the label.
    PrefMoveAddLabel = "Append the torrent's label";
    PrefTorrentLocation = "Location of .torrents";
    PrefStoreTorrents = "Store .torrents in:";
    PrefMoveTorrents = "Move .torrents for finished jobs to:";
    PrefAutoload = "Automatically load .torrents from:";
    PrefAutoloadDelete = "Delete loaded .torrents";
    PrefListeningPort = "Listening Port";
    PrefPort = "Port used for incoming connections:";
    PrefRandomPort = "Random port";
    PrefRandomize = "Randomize port each start";
    PrefUpnp = "Enable UPnP port mapping";
    PrefNatPmp = "Enable NAT-PMP port mapping";
    PrefProxyServer = "Proxy Server";
    PrefProxyType = "Type:";
    PrefProxy = "Proxy:";
    PrefProxyPort = "Port:";
    PrefAuthentication = "Authentication";
    PrefUsername = "Username:";
    PrefPassword = "Password:";
    /// The Web UI's Preferences: needed to change the login or the security settings.
    PrefCurrentPassword = "Current password:";
    PrefProxyNames = "Resolve hostnames through proxy";
    PrefProxyPeers = "Use proxy for peer-to-peer connections";
    PrefProxyTrackers = "Use proxy for tracker connections";
    /// Proxy type: no proxy.
    PrefProxyNone = "(none)";
    /// Proxy types: protocol names, usually kept as they are.
    ProxySocks4 = "Socks4";
    ProxySocks5 = "Socks5";
    ProxyHttp = "HTTP";
    PrefUpLimiting = "Global Upload Rate Limiting";
    PrefUpRate = "Maximum upload rate (kB/s): [0: unlimited]";
    PrefDownLimiting = "Global Download Rate Limiting";
    PrefDownRate = "Maximum download rate (kB/s): [0: unlimited]";
    PrefConnections = "Number of Connections";
    PrefMaxConnections = "Global maximum number of connections:";
    PrefPeersPerTorrent = "Maximum number of connected peers per torrent: [0: no limit]";
    PrefSlotsPerTorrent = "Number of upload slots per torrent: [0: no limit]";
    PrefGlobalSlots = "Global number of upload slots: [-1: no limit]";
    PrefPerTorrentNote = "The per-torrent limits apply to torrents added from now on.";
    PrefBasicFeatures = "Basic BitTorrent Features";
    PrefDht = "Enable DHT Network";
    PrefLsd = "Enable Local Peer Discovery";
    PrefPex = "Enable Peer Exchange";
    PrefUtp = "Enable uTP (UDP transport with bandwidth management)";
    PrefEncryption = "Protocol Encryption";
    PrefOutgoing = "Outgoing:";
    PrefLegacy = "Allow incoming legacy connections";
    /// Encryption choices.
    EncDisabled = "Disabled";
    EncEnabled = "Enabled";
    EncForced = "Forced";
    PrefIpFilter = "IP Filter";
    PrefIpFilterEnable = "Enable IP filter";
    PrefReload = "Reload";
    PrefIpFilterLoaded = "{count} rules loaded.";
    /// The path field is empty: {path} is the file used instead.
    PrefIpFilterDefault = "Empty: {path}";
    PrefIpFilterOff = "The IP filter is off.";
    PrefQueue = "Queue Settings";
    PrefActive = "Maximum number of active torrents (upload or download): [-1: no limit]";
    PrefActiveDownloads = "Maximum number of active downloads: [-1: no limit]";
    PrefActiveSeeds = "Maximum number of active seeds: [-1: no limit]";
    PrefSlow = "Don't count slow torrents toward these limits";
    PrefSeedingGoal = "Seeding Goal";
    PrefRatio = "Minimum ratio (%): [0: no ratio goal]";
    PrefSeedTime = "Minimum seeding time (minutes): [0: no time goal]";
    PrefGoalReached = "When a torrent reaches the seeding goal (forced torrents never do):";
    PrefGoalLimit = "Limit the upload rate to (kB/s): [0: stop]";
    PrefGoalLimitInstead = "Limit the upload rate instead of stopping the torrent";
    PrefAltUpRate = "Alternative upload rate (kB/s): [0: unlimited]";
    PrefAltDownRate = "Alternative download rate (kB/s): [0: unlimited]";
    PrefUseAltRates = "Use the alternative rate limits now";
    PrefScheduler = "Enable Scheduler";
    PrefSchedTable = "Scheduler Table";
    PrefSchedSettings = "Scheduler Settings";
    PrefSchedUp = "Limited upload rate (kB/s): [0: unlimited]";
    PrefSchedDown = "Limited download rate (kB/s): [0: unlimited]";
    PrefSchedDht = "Disable DHT when turning off";
    /// The scheduler grid's tooltip: {day}, {from} and {to} hours, {mode}.
    SchedHover = "{day} {from}:00 - {to}:00: {mode}";
    PrefWebUi = "Enable Web UI";
    /// Preferences > Web UI (Windows): this window serves the Web UI itself (D40).
    PrefWebUiNote = "The Web UI shows this window's torrents in a browser while Rookparcel is running (close to the tray to keep it up). Connections from other computers also need a Windows Firewall rule for rookparcel.exe. The page's RSS Downloader edits this window's feeds and filters.";
    /// Beside the password field: an empty field keeps the stored password.
    PrefWebUiPassKeep = "(blank: unchanged)";
    PrefWebUiConnectivity = "Connectivity";
    PrefWebUiPort = "Listening port:";
    PrefWebUiRemote = "Allow connections from other computers";
    PrefWebUiHttps = "Use HTTPS (self-signed certificate)";
    PrefWebUiOpen = "Open Web UI";
    /// The Web UI page's status line; {url} is the page's address.
    WebUiRunning = "The Web UI is running: {url}";
    WebUiStopped = "The Web UI is off.";
    WebUiNeedLogin = "Please enter a user name and a password for the Web UI.";
    /// {error}: why the server could not start (the port is in use, a bad certificate).
    WebUiFailed = "The Web UI could not start: {error}";
    /// `WebUiFailed`'s `{error}` when its port is taken (D42).
    WebUiPortInUse = "port {port} is in use, perhaps by another Rookparcel profile";
    /// A window driving a rookparcel-nox elsewhere (File > Connect to Remote) has no engine to serve.
    WebUiRemoteWindow = "A remote window cannot serve the Web UI; use the local window's Preferences.";
    PrefFilter = "Filter:";
    PrefValue = "Value:";
    PrefTrue = "True";
    PrefFalse = "False";
    PrefSet = "Set";
    PrefReset = "Reset";
    /// Preferences > Advanced: the list's columns.
    ColValue = "Value";
    PrefAdvancedNote = "libtorrent setting names; * marks a changed value. Values are applied with OK or Apply.";
    PrefDiskQueue = "Disk Queue and Files";
    PrefDiskWriteQueue = "Disk write queue (kB):";
    PrefOpenFiles = "Maximum number of open files:";
    PrefAioThreads = "Disk I/O threads (used from the next start):";
    /// Kept for libtorrent compatibility; this version reads it nowhere.
    PrefHashThreads = "Hashing threads (not used):";
    PrefBadNumber = "Please enter a number from {min} to {max}.";
    PrefNoProxyHost = "Please enter the proxy server's address.";
    PrefNoProxyPort = "Please enter the proxy server's port.";
    PrefNoDownloadDir = "Please choose a folder for new downloads.";
    PrefNoCompletedDir = "Please choose a folder for completed downloads.";
    PrefNoStoreDir = "Please choose a folder to store .torrents in.";
    PrefNoFinishedDir = "Please choose a folder for the .torrents of finished jobs.";
    PrefNoAutoloadDir = "Please choose a folder to load .torrents from.";
    PickDownloadDir = "Select the folder for new downloads";
    PickCompletedDir = "Select the folder for completed downloads";
    PickStoreDir = "Select the folder to store .torrents in";
    PickFinishedDir = "Select the folder for the .torrents of finished jobs";
    PickAutoloadDir = "Select the folder to load .torrents from";
    /// The language list's first entry: the system's language, {language} in its own name.
    LangSystemDefault = "System default ({language})";
    /// After the language was changed in Preferences.
    LangRestart = "Rookparcel shows the new language from its next start.";
    /// Preferences > Disk Cache: the read cache's group and controls; the two ending in ":"
    /// have a number field beside them.
    PrefReadCache = "Disk Read Cache";
    PrefReadCacheEnable = "Enable caching of disk reads";
    PrefCacheOverride = "Override the automatic cache size (MB):";
    PrefCacheExpire = "Remove blocks unused for more than (seconds):";
    /// {mb} is the automatic cache size in MB. read_cache_line_size is a setting's name: keep it.
    PrefReadCacheNote = "Rookparcel caches disk reads: a block a peer asks for is read together with the blocks around it (read_cache_line_size on the Advanced page), which saves disk seeks when seeding. The automatic size is {mb} MB. Writes are left to the system's cache.";
    /// Preferences > Bandwidth: µTorrent 2.2.1's group and its boxes, then Rookparcel's own.
    PrefRateOptions = "Global Rate Limit Options";
    PrefRateOverhead = "Apply rate limit to transport overhead";
    PrefRateUtp = "Apply rate limit to uTP connections";
    /// Lowers the upload and download speeds while they make the internet line's queue (and
    /// so the ping of games and calls) grow.
    PrefKeepPingLow = "Keep ping low (limit speeds automatically)";
    /// auto_limit_probe_hosts is a setting's name: keep it. SQM and QoS are router features.
    PrefKeepPingLowNote = "Keep ping low pings the first router past your line, or else the hosts in auto_limit_probe_hosts on the Advanced page, whose operators see those pings. A router with SQM or gaming QoS does this better, for every device.";
    /// Preferences > Advanced > Disk I/O (a Rookparcel page beside Disk Cache): how checking
    /// (hashing a torrent's files) and the disk threads treat the drives. I/O is input/output.
    PageDiskIo = "Disk I/O";
    /// Group title: checking a torrent's files against their hashes (Force Re-Check).
    PrefChecking = "Checking";
    /// Checks run at the system's background disk priority and wait while other programs use
    /// the disk.
    PrefCheckGently = "Check torrents gently (low priority, give way to other programs)";
    /// A check box with a number field beside it.
    PrefCheckRateLimit = "Limit checking to (MB/s):";
    /// A number field: how many checks run at once, each on its own drive.
    PrefActiveChecking = "Drives checked at once (one torrent per drive):";
    /// Group title.
    PrefDrives = "Drives";
    /// Disk work is tuned per drive by its detected type: a solid-state drive, a hard disk or
    /// a network share.
    PrefTuneForDrive = "Tune for the drive type (SSD, hard disk, network)";
    /// Group title: while a drive answers more slowly than usual.
    PrefDiskBusy = "When the Disk Is Busy";
    PrefDiskSlowDownloads = "Slow downloads down (disk writes)";
    PrefDiskSlowUploads = "Slow uploads down (disk reads)";
    /// drive_overrides is a setting's name: keep it.
    PrefDiskIoNote = "To correct a drive whose type is detected wrongly, set it in drive_overrides on the Advanced page.";
    /// The one line at the bottom of Preferences saying where help is (D55): pointing at a
    /// setting shows its help in the box above, F1 on a setting shows it beside the setting.
    PrefHelpHint = "Hover a setting for help, F1 for the guide";
    /// Preferences > Connection, group title: torrent traffic uses only the network adapter
    /// chosen below (a VPN, typically) and nothing else.
    PrefUseNetwork = "Use Only This Network";
    /// The drop-down of NetRuleOff, NetRuleVpn, NetRuleRange and the computer's network
    /// adapters by name.
    PrefNetworkRule = "Network:";
    /// The text box for an address range such as 10.2.0.0/16, used with NetRuleRange.
    PrefNetworkRange = "Address range:";
    PrefNetworkPause = "Pause torrents while this network is missing";
    /// The drop-down's first choice: no binding, any network may be used.
    NetRuleOff = "Any network (off)";
    /// Whatever adapter looks like a VPN tunnel (WireGuard, OpenVPN, the VPN app's own).
    NetRuleVpn = "Any VPN adapter";
    /// The addresses in the range typed below, on whatever adapter has them.
    NetRuleRange = "Address range (below)";
    /// An adapter the setting names that is not on the computer now; {name} is its name or id.
    NetAdapterMissing = "{name} (not connected)";
    /// {range} is what the user typed.
    PrefBadRange = "\"{range}\" is not an address range such as 10.2.0.0/16 or fd00::/8.";

    // ---- RSS Downloader

    RssTitle = "RSS Downloader";
    RssAddFeed = "Add RSS Feed";
    RssEditFeed = "Edit RSS Feed";
    RssFeedUrl = "Feed &URL:";
    RssAlias = "Custom &Alias:";
    RssSubscription = "Subscription";
    RssNoAuto = "Do &not automatically download all items";
    RssAuto = "Automatically &download all items published in feed";
    RssSmartAuto = "Use &smart episode filter";
    RssNoUrl = "Please enter the URL of the feed.";
    RssFeeds = "Feeds";
    RssFeedsAdd = "&Add...";
    RssFeedsEdit = "&Edit...";
    RssFeedsRemove = "Remo&ve";
    RssFeedsUpdate = "&Update";
    RssFilterAdd = "Add";
    RssFilterRemove = "Remove";
    RssFilterUp = "Up";
    RssFilterDown = "Down";
    RssName = "Name:";
    RssFilter = "Filter:";
    /// Titles matching this are skipped.
    RssNot = "Not:";
    RssSaveIn = "Save in:";
    RssFeed = "Feed:";
    RssQuality = "Quality:";
    RssEpisodes = "Episode number:";
    RssMinInterval = "Minimum interval:";
    RssLabel = "Label:";
    RssMatching = "Matching items in the feeds:";
    /// An example of the episode number syntax; keep 1x12-14.
    RssEpisodesExample = "[ex. 1x12-14]";
    RssSmart = "S&mart ep. filter";
    RssHighPriority = "Give download &highest priority";
    RssPaused = "Add torrent in &paused state";
    RssClearHistory = "&Clear History";
    RssTabFilters = "Filters";
    RssTabHistory = "History";
    RssColFilter = "Filter";
    RssAllFeeds = "All Feeds";
    /// {count} is the number of feeds.
    RssFeedCount = "Feeds: {count}";
    RssPickSaveDir = "Select the folder to save the downloads in";

    // ---- The Web UI (rookparcel-nox): the page, its dialogs and its Preferences

    WebUserName = "User name";
    WebPassword = "Password";
    WebLogIn = "Log in";
    WebLogOut = "Log out";
    WebWrongLogin = "Wrong user name or password.";
    WebCategories = "Categories";
    /// Short toolbar buttons.
    WebAdd = "Add";
    WebAddUrl = "Add URL";
    WebUp = "Up";
    WebDown = "Down";
    WebRss = "RSS";
    /// Ends the phone's selection mode.
    WebDone = "Done";
    WebEndSelection = "End the selection";
    WebMore = "More";
    WebSearch = "Search";
    WebSearchTorrents = "Search torrents";
    WebCloseDetails = "Close details";
    WebActions = "Actions";
    WebDetails = "Details";
    WebPrefPages = "Preference pages";
    WebConnecting = "Connecting…";
    /// {secs} seconds.
    WebReconnecting = "Disconnected, reconnecting in {secs} s";
    WebNoMatch = "No torrents match.";
    WebNoTorrents = "No torrents. Add one with the toolbar.";
    /// The status bar: {count} torrents selected.
    WebSelected = "Selected: {count}";
    WebSortBy = "Sort By";
    WebSortDescending = "Sort Descending";
    WebResetColumns = "Reset Columns";
    /// Why a menu item is grey.
    WebNotFromBrowser = "not from a browser";
    WebSetLocation = "Set Download Location\n\nMove the selected torrents' files to this folder on the server:";
    WebPiecesSummary = "{count} pieces, {done} done.";
    WebPiecesSummaryDownloading = "{count} pieces, {done} done, {downloading} downloading.";
    /// A piece's tooltip.
    WebBlocksDone = "{done} of {count} blocks done";
    WebRssItems = "Items";
    /// A feed that is switched off.
    WebFeedDisabled = "{name} (disabled)";
    WebFeedNotSaved = "The feed could not be saved.";
    WebRssSaveIn = "Save in (a folder on the server; empty for the default download folder):";
    /// A password field left empty keeps the password.
    WebUnchanged = "(unchanged)";
    WebPrefsNotApplied = "The preferences could not be applied.";
    WebApplied = "Applied.";
    WebPropsNotSet = "The properties could not be set.";
    /// Preferences > Web UI.
    WebListenAddress = "Listening address:";
    WebBypassLocal = "Bypass authentication for clients on localhost";
    WebHostHeader = "Enable Host header validation";
    WebDomains = "Server domains (separate multiple entries with a semicolon):";
    WebCsrf = "Enable Cross-Site Request Forgery (CSRF) protection";
    WebBanAfter = "Ban client after consecutive failures:";
    WebBanFor = "Ban for (seconds):";
    WebSessionTimeout = "Session timeout (seconds):";
    PrefConnectivity = "Connectivity";
    PrefSecurity = "Security";
    PrefAltRates = "Alternative Rate Limits";
    /// {path} is the block list used when the field is empty.
    PrefIpFilterEmptyPath = "An empty path loads {path}.";
    PrefDesktopOnly = "File associations and autostart belong to the desktop client.";
    PrefAdvancedNoteWeb = "libtorrent setting names; changed values are shown in bold. Values are applied with OK or Apply.";
    PrefNoUsername = "Please enter a user name.";
    PrefNeedCurrentPassword = "Enter the current password to change the login or the Web UI's security settings.";
    /// "Bearer" and the header name are protocol words: keep them as they are.
    WebApiKeyEnabled = "Accept an API key (Authorization: Bearer)";
    /// A password field: left empty, the key stays as it is.
    WebApiKey = "API key (at least 16 characters):";
    WebAuthSubnets = "Bypass authentication for clients in these subnets (such as 192.168.1.0/24; separate multiple entries with a semicolon):";
    /// {count} is the shortest length taken.
    PrefApiKeyShort = "The API key must be at least {count} characters long.";
    /// {entry} is the entry of the subnet list that does not parse.
    PrefBadSubnet = "\"{entry}\" is not an IP address or a subnet (such as 192.168.1.0/24).";
    /// {count} is the number of hours in a week.
    SettingBadSchedule = "expected {count} hours, each 0 (full speed), 1 (turn off), 2 (limited) or 3 (seeding only)";

    // ---- Create New Torrent: choices and errors

    /// Piece size: pick one from the total size.
    CreateAutoDetect = "(auto detect)";
    CreateHybrid = "Hybrid (v1 + v2)";
    CreateV1 = "v1";
    CreateV2Only = "v2 only";
    CreateNoSource = "Please select a file or a directory to create the torrent from.";
    CreateNoName = "The source has no usable name.";
    /// {path} is a file or folder.
    CreateNoFiles = "{path}: no files to put in the torrent (empty, or all skipped).";
    CreateDoesNotLoad = "The new torrent does not load: {error}";
    CreateNotUnicode = "{path}: the name is not valid Unicode";

    // ---- Torrent Properties: errors

    PropsBadUploadRate = "The maximum upload rate must be a whole number from 0 to {max}.";
    PropsBadDownloadRate = "The maximum download rate must be a whole number from 0 to {max}.";
    PropsBadUploadSlots = "The number of upload slots must be a whole number from 0 to {max}.";
    PropsBadRatio = "The minimum ratio must be a whole number from 0 to {max}.";
    PropsBadSeedTime = "The minimum seeding time must be a whole number from 0 to {max}.";
    PropsBadNumber = "The value must be a whole number from 0 to {max}.";
    PropsBadTracker = "\"{line}\" is not a tracker URL (http://, https:// or udp://).";

    // ---- Options > Auto Shutdown

    ShutdownDisabled = "&Disabled";
    ShutdownQuitDownloads = "&Quit When Downloads Complete";
    ShutdownQuitEverything = "Quit When &Everything Completes";
    ShutdownStandbyDownloads = "&Standby When Downloads Complete";
    ShutdownStandbyEverything = "Standby &When Everything Completes";
    ShutdownHibernateDownloads = "&Hibernate When Downloads Complete";
    ShutdownHibernateEverything = "Hibernate When E&verything Completes";
    ShutdownShutDownDownloads = "Shut Down When D&ownloads Complete";
    ShutdownShutDownEverything = "Shut Down When Everything &Completes";

    // ---- The scheduler

    /// Days of the week, short, Monday first.
    DayMon = "Mon";
    DayTue = "Tue";
    DayWed = "Wed";
    DayThu = "Thu";
    DayFri = "Fri";
    DaySat = "Sat";
    DaySun = "Sun";
    SchedFullSpeed = "Full speed";
    SchedLimited = "Limited";
    SchedTurnOff = "Turn off";
    SchedSeedingOnly = "Seeding only";
    /// The status bar while the scheduler limits the session.
    SchedStatusLimited = "Scheduler: limited";
    SchedStatusOff = "Scheduler: turned off";
    SchedStatusSeedingOnly = "Scheduler: seeding only";

    // ---- Add New Torrent

    /// {selected} and {total} are sizes like "1.20 GB".
    AddSizeSelected = "{selected} of {total} selected";

    // ---- .torrent folders (Logger lines)

    /// {name} is a torrent's name, {path} a file.
    FoldersStored = "{name}: .torrent stored as {path}";
    FoldersStoreFailed = "{name}: could not store the .torrent in {dir}: {error}";
    FoldersNotLoaded = "{path}: not loaded: {error}";
    FoldersMoved = "{name}: .torrent moved to {path}";
    FoldersMoveFailed = "{name}: could not move the .torrent to {dir}: {error}";
    FoldersLoadedRenamed = "{name}: loaded from {path}, renamed to {to}";
    FoldersLoadedDeleted = "{name}: loaded from {path}, deleted";
    FoldersLoadedKept = "{name}: loaded from {path}, which could not be renamed or deleted: {error}";

    // ---- Preferences > Advanced: errors

    /// {value} is what was typed.
    SettingNotBool = "expected true or false, not \"{value}\"";
    SettingOutOfRange = "{value} is outside {min}..{max}";
    SettingNotNumber = "\"{value}\" is not a number";
    /// {options} lists the choices.
    SettingNotOneOf = "\"{value}\" is not one of {options}";
    /// {name} is a setting's name.
    SettingUnknown = "unknown setting \"{name}\"";

    // ---- RSS

    RssDisabled = "Disabled";
    /// {error} is a network or feed error.
    RssError = "Error: {error}";
    /// {time} is a date and time, {count} the number of items in the feed.
    RssUpdated = "Updated {time}, items: {count}";
    RssNotUpdated = "Not updated yet";
    /// A filter's minimum interval that is not in the list.
    RssSeconds = "{secs} s";
    /// Quality: any.
    RssQualityAll = "All";
    /// The name of a filter just added.
    RssNewFilter = "New filter";
    RssInterval0 = "0 (Ignore)";
    RssInterval2h = "2 hours";
    RssInterval5h = "5 hours";
    RssInterval10h = "10 hours";
    RssInterval12h = "12 hours";
    RssInterval1d = "1 day";
    RssInterval2d = "2 days";
    RssInterval3d = "3 days";
    RssInterval4d = "4 days";
    RssInterval1w = "1 week";
    RssInterval2w = "2 weeks";
    RssInterval3w = "3 weeks";
    RssInterval1mo = "1 month";
    /// Logger lines; {name} is a feed's name, {title} an item's title.
    RssLogError = "RSS: {name}: {error}";
    RssLogDownloading = "RSS: {name}: downloading {title}";
    RssLogDownloadFailed = "RSS: could not download {title}: {error}";
    /// {url} is a web address.
    RssErrRedirect = "{url}: unsupported redirect";
    RssErrNotTorrent = "{url}: not a .torrent file";
    RssErrFeedRedirect = "{url}: redirected away from the feed";
    RssErrNotFeed = "{url}: not an RSS or Atom feed";

    // ---- The toolbar's search box (it filters the torrent list by name)

    /// The empty search box's grey hint.
    SearchCue = "Search torrent names";

    // ---- The Files tab's quick filter (D50)

    /// The empty filter box's grey hint, at the Files tab's top right.
    FilesFilterCue = "Filter files";
    /// Beside the filter box while it filters: {shown} files listed of the torrent's {total}.
    FilesFilterCount = "{shown} of {total} files";
    /// The tooltip of the × that empties the filter box (Win32; GTK's search entry has its own).
    FilesFilterClear = "Clear the filter (Esc)";

    // ---- File > Import: torrents from another client; {client} is qBittorrent or µTorrent

    MenuImportQbt = "Import from &qBittorrent...";
    MenuImportUt = "Import from &µTorrent...";
    /// The folder picker's title when the client's usual folder is not there.
    ImportPickFolder = "Select the {client} folder";
    ImportRunning = "{client} is running. Close it, then import again: it rewrites its files when it exits.";
    ImportNoneFound = "No {client} torrents were found in {dir}.";
    /// The question before importing; {count} is a number.
    ImportConfirm = "Import {count} torrents from {client}?\n\nTheir data stays where it is and {client}'s files are not changed. Torrents already in the list are left out.";
    /// The answer after importing; {added} is a number.
    ImportDone = "Added {added} torrents from {client}.";
    /// Follows ImportDone when some were not imported; {skipped} is a number.
    ImportLeftOut = "{skipped} were left out; the Logger tab lists why.";
    /// Logger lines; {name} is a torrent's name.
    LogImported = "Imported from {client}: {name}";
    LogImportSkipped = "Not imported from {client}: {name}: {reason}";
    /// Reasons a torrent is not imported.
    ImportInList = "already in the list";
    ImportNoTorrentFile = "its .torrent file is missing";
    ImportBadRecord = "its resume record cannot be read";
    ImportBadTorrent = "its .torrent file cannot be read: {error}";

    // ---- Torrent search through the user's Prowlarr or Jackett (Torznab search sources)

    /// The search window's title and the toolbar button's tooltip.
    SearchTitle = "Search Torrents";
    /// The Web UI toolbar button that opens the search window (short).
    SearchFind = "Find";
    /// The empty query box's grey hint.
    SearchQueryCue = "Search with Prowlarr or Jackett";
    /// The category list: Newznab's main categories.
    SearchAllCategories = "All categories";
    SearchCatMovies = "Movies";
    SearchCatAudio = "Audio";
    /// Software and games for computers.
    SearchCatPc = "PC";
    SearchCatTv = "TV";
    SearchCatBooks = "Books";
    SearchCatOther = "Other";
    /// The source list's first choice: search every enabled source.
    SearchAllSources = "All sources";
    /// Result list columns (Name, Size, Seeds and Peers are the torrent list's).
    SearchColCategory = "Category";
    /// How long ago the indexer published the torrent.
    SearchColAge = "Age";
    /// Which search source returned the result.
    SearchColSource = "Source";
    /// A source's status line; {source} is its name.
    SearchSearching = "{source}: searching...";
    /// {count} is a number.
    SearchFound = "{source}: {count} results";
    /// {error} is the reason, often in English.
    SearchFailed = "{source}: {error}";
    SearchStopped = "{source}: stopped";
    /// {count} is a number.
    SearchResultCount = "Results: {count}";
    SearchNoSources = "No search sources yet: add your Prowlarr or Jackett under Manage Sources.";
    /// A torrent added from the results; {name} is its name.
    SearchAdded = "Added: {name}";
    /// Adding a result refused: the .torrent or magnet its link gave has another info-hash than
    /// the result; {url} is the link (may be empty).
    SearchErrHashMismatch = "{url}: not the torrent of this result (its info-hash differs)";
    SearchManage = "Manage Sources...";
    /// The window listing the search sources.
    SearchSourcesTitle = "Search Sources";
    /// Asks the source for its categories, to check its URL and API key.
    SearchTest = "Test";
    SearchTesting = "{source}: testing...";
    /// {count} is a number.
    SearchTestOk = "{source}: OK, {count} categories";
    SearchAddSource = "Add Search Source";
    SearchEditSource = "Edit Search Source";
    SearchSourceName = "Name:";
    /// Torznab is the protocol's name: do not translate it.
    SearchSourceUrl = "Torznab URL:";
    /// Keep the URLs as they are.
    SearchSourceUrlHint = "Prowlarr: http://host:9696/1/api (one per indexer). Jackett: http://host:9117/api/v2.0/indexers/all/results/torznab/";
    SearchSourceKey = "API key:";
    /// The empty API key box's grey hint when editing a source: the stored key stays.
    SearchSourceKeyKept = "Unchanged";
    SearchSourceEnabled = "Search this source";
    SearchNoUrl = "Enter the source's Torznab URL.";
    /// {name} is a search source's name.
    AskRemoveSource = "Remove the search source {name}?";
    /// The Web UI asks for its own password before a source is added or its address changed.
    WebSearchPassword = "Current Web UI password:";
    WebSearchNeedPassword = "Enter the current Web UI password to add a search source or change its URL or API key.";

    // Search (Win32)

    /// The toolbar search box's drop-down: typing filters the torrent list by name.
    SearchFilterTorrents = "Filter torrents";
    /// The toolbar search box's grey hint when Enter searches one source; {source} is its name.
    SearchCueSource = "Search {source}";
    /// The toolbar search box's grey hint when Enter searches every source.
    SearchCueAll = "Search all sources";
    /// The tooltip of the arrow beside the toolbar search box, which opens its drop-down.
    SearchModeTip = "Filter the list or search your sources";

    // The search helper (D51): Jackett, downloaded at the user's click

    /// The search source the search helper adds. Keep "(all)": it searches all its sites.
    SearchHelperSource = "Search helper (all)";
    /// The button in Search Sources. Jackett is the program's name: do not translate it.
    SearchHelperGet = "Get the Search Helper (Jackett)...";
    SearchHelperChecking = "Looking for the newest search helper...";
    /// The box before the download. Keep "GPL-2.0" and the address as they are. {version} is
    /// the release (v0.24.3009), {size} its size (57.0 MB), {folder} where it goes.
    SearchHelperConfirm = "The search helper is Jackett, a separate program by its own authors under the GPL-2.0 licence, downloaded from github.com/Jackett/Jackett ({version}, {size}).\n\nIt goes into {folder} and runs on this computer only, only while you search. You then pick the public sites it searches.\n\nDownload it now?";
    /// {version} is the release (v0.24.3009).
    SearchHelperDownloading = "Downloading the search helper {version}...";
    /// {error} is the reason, often in English.
    SearchHelperDownloadFailed = "Could not get the search helper: {error}";
    SearchHelperNotInstalled = "The search helper is not installed.";
    /// {error} is the reason, often in English.
    SearchHelperFailed = "The search helper did not start: {error}";
    SearchHelperStarting = "Starting the search helper...";
    /// The button that opens the list of the helper's public sites.
    SearchHelperSites = "Pick Sites...";
    SearchHelperSitesTitle = "Search Helper Sites";
    SearchHelperSitesHint = "Tick the public sites the search helper searches; nothing is ticked for you. For private sites and their logins, open the helper's own page.";
    /// A column of the sites list: the site's language.
    SearchHelperColLanguage = "Language";
    /// A column of the sites list: public, semi-private or private.
    SearchHelperColType = "Type";
    /// A column of the sites list: why a site cannot be ticked.
    SearchHelperColNote = "Note";
    /// FlareSolverr is a program's name: do not translate it.
    SearchHelperNeedsFlaresolverr = "Needs FlareSolverr, which the helper does not include";
    SearchHelperSaving = "Setting up the sites...";
    /// {errors} lists the sites and their errors, one per line.
    SearchHelperSitesFailed = "Some sites could not be set up:\n{errors}";
    /// {count} is a number.
    SearchHelperSitesDone = "Search helper: {count} sites ticked.";
    SearchHelperOpenPage = "Open the Helper's Page";
    /// {password} is the generated password, shown so it can be typed on the page.
    SearchHelperPagePassword = "The helper's page opens in your browser. It asks for this password (copied to the clipboard):\n\n{password}";
    SearchHelperUpdate = "Update the Search Helper";
    /// {version} is the release (v0.24.3009).
    SearchHelperUpToDate = "The search helper is up to date ({version}).";
    /// {current} and {version} are releases, {size} the download's size.
    SearchHelperUpdateConfirm = "Update the search helper from {current} to {version} ({size})?";
    SearchHelperRemove = "Remove the Search Helper";
    /// {folder} is the folder deleted.
    SearchHelperRemoveConfirm = "Remove the search helper and its search source? Its folder is deleted:\n{folder}";
    /// The status line after an install or update; {version} is the release.
    SearchHelperInstalled = "Search helper {version} is installed.";
    SearchHelperRemoved = "The search helper was removed.";
    // The search helper in the Web UI (D51): it runs on the server, the browser may be elsewhere
    /// The download makes the server download and run a program: the Web UI asks for its password.
    WebHelperNeedPassword = "Enter the current Web UI password to download and run the search helper.";
    /// Manage Sources in a browser on another computer: the helper listens on the server only.
    WebHelperPageHostOnly = "The helper's own page opens only in a browser on the computer Rookparcel runs on, where the helper listens.";
    /// After "Open the Helper's Page": a link to the page and the password follow.
    WebHelperPagePassword = "The search helper runs. Its page asks for this password:";
    WebHelperBusy = "The search helper is busy with another step; try again when it is done.";
    /// A download or a step the user cancelled.
    WebHelperCancelled = "Cancelled.";
    // ---- Options > Setup Guide (D53): a few questions at a profile's first start
    /// The guide's window title.
    GuideTitle = "Setup Guide";
    /// Above each page's title; {step} and {count} are numbers.
    GuideStep = "Step {step} of {count}";
    /// The guide's buttons; Skip closes it and keeps the default settings.
    GuideBack = "< &Back";
    GuideNext = "&Next >";
    GuideFinish = "&Finish";
    GuideSkip = "&Skip";
    /// Page 1: the download folder.
    GuidePageDownloads = "Where downloads go";
    GuideDownloadsText = "Pick the folder new downloads are saved in. Finished downloads can also move to another folder by themselves, from a fast disk to a big one, for example. Preferences > Directories has more.";
    /// Page 2: the router's connection table.
    GuidePageRouter = "Your router";
    GuideRouterText = "Every connection to a peer takes a place in your router's connection table, and DHT and uTP add many short ones. When a small table fills up, the router drops connections, slows down the whole house or restarts. Older routers and the basic boxes of internet providers have small tables.";
    /// The question above the two answers below.
    GuideRouter = "Which router do you have?";
    GuideRouterModern = "Modern (bought after about 2015), or not sure";
    GuideRouterOlder = "Older, or the internet provider's basic box";
    /// {total}, {per_torrent} and {per_second} are numbers.
    GuideRouterNote = "The older choice lowers the connections to {total} in all, {per_torrent} per torrent and {per_second} new ones a second: a starting point that Preferences > Bandwidth and Advanced change later. For games and calls, Keep ping low holds the speeds down while the line is busy.";
    /// Page 3: smart disk I/O.
    GuidePageDisks = "Your disks";
    GuideDisksText = "Smart disk I/O keeps the computer responsive while Rookparcel reads and writes: checks run at the system's background priority, each drive is tuned for its type, and transfers slow down while a disk answers slowly. Checks take longer while something else uses the disk.";
    GuideSmartDisk = "Use smart disk I/O";
    GuideDisksNote = "Gentle checks and per-drive tuning are on from the start; the box adds the slow-down while a disk is busy. Preferences > Advanced > Disk I/O sets each one.";
    /// Page 4: starting with the system and opening .torrent files and magnet: links.
    GuidePageComputer = "This computer";
    GuideComputerText = "Rookparcel can start when you sign in, and open .torrent files and magnet: links from your browser and file manager. Preferences > General changes this later.";

    // ---- macOS menus: the Mac GUI's own items, as macOS names them (no &: Mac menus have no mnemonics)

    /// The menu bar's Edit menu.
    MacMenuEdit = "Edit";
    /// The menu bar's View menu.
    MacMenuView = "View";
    /// The menu bar's menu of the selected torrents' commands (Start, Stop, Remove, ...).
    MacMenuTorrent = "Torrent";
    /// The menu bar's Window menu.
    MacMenuWindow = "Window";
    /// The application menu's submenu of the system's services.
    MacServices = "Services";
    /// Hides every other application's windows.
    MacHideOthers = "Hide Others";
    /// Shows every application's windows again.
    MacShowAll = "Show All";
    MacQuit = "Quit Rookparcel";
    MacUndo = "Undo";
    MacRedo = "Redo";
    MacCut = "Cut";
    MacCopy = "Copy";
    MacPaste = "Paste";
    MacDelete = "Delete";
    /// Edit menu: moves to the toolbar's search box.
    MacFind = "Find...";
    /// View menu, while the toolbar is shown (Show Toolbar is MenuShowToolbar).
    MacHideToolbar = "Hide Toolbar";
    /// View menu: the window where toolbar buttons are added, removed and moved.
    MacCustomizeToolbar = "Customize Toolbar...";
    /// View menu: the list of categories, labels and feeds at the window's left.
    MacShowSidebar = "Show Sidebar";
    MacHideSidebar = "Hide Sidebar";
    /// View menu, while the detail tabs are shown (Show Detailed Info is MenuShowDetails).
    MacHideDetails = "Hide Detailed Info";
    /// View menu, while the status bar is shown (Show Status Bar is MenuShowStatus).
    MacHideStatusBar = "Hide Status Bar";
    MacEnterFullScreen = "Enter Full Screen";
    /// Window menu: puts the window into the Dock.
    MacMinimize = "Minimize";
    /// Window menu: switches the window between its size and the screen's.
    MacZoom = "Zoom";
    MacBringAllToFront = "Bring All to Front";
    MacCloseWindow = "Close Window";
}
