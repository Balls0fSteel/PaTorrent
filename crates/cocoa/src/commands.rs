//! Every action of the GUI as one value (docs/MACOS_GUI.md, convention 2): the menus, the
//! toolbar, buttons, the Dock menu and the status item all name a [`Command`], and `app::run`
//! runs it in one place. A menu item or toolbar item carries its command as its `tag`
//! ([`Command::tag`]), which `RPTarget` turns back into the command.
//!
//! Commands with a value (a label, a profile, a speed limit, an Auto Shutdown mode, a search
//! source) carry an index into a list their menu was built from.
//!
//! Pure Rust, compiled and tested on every system.

/// One action.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Command {
    // ---- the application menu
    About,
    CheckUpdates,
    Preferences,
    SetupGuide,
    Quit,
    // ---- File
    AddTorrent,
    AddUrl,
    CreateTorrent,
    ImportQbt,
    ImportUt,
    /// Starts the profile at this index of `profile::menu` (or brings this one forward).
    Profile(u16),
    NewProfile,
    ProfileFolder,
    ConnectRemote,
    // ---- Edit (the others are AppKit's own first-responder actions)
    /// The toolbar's search field; in the Files tab its filter (slice D).
    Find,
    // ---- View
    ToggleToolbar,
    ToggleSidebar,
    ToggleDetails,
    ToggleStatusBar,
    // ---- Torrent (also the list's context menu)
    OpenFolder,
    ForceStart,
    Start,
    Pause,
    Stop,
    QueueUp,
    QueueDown,
    QueueTop,
    QueueBottom,
    Remove,
    RemoveAndDelete,
    Recheck,
    UpdateTracker,
    SetLocation,
    InitialSeeding,
    /// An index into `actions::LIMITS`.
    DownLimit(u16),
    /// An index into `actions::LIMITS`.
    UpLimit(u16),
    NewLabel,
    RemoveLabel,
    /// An index into the label list the menu was built from (`app::labels`).
    Label(u16),
    Properties,
    // ---- Options
    Rss,
    /// An index into `auto_shutdown::Mode::MENU`.
    AutoShutdown(u16),
    // ---- Help
    Website,
    Notices,
    // ---- the Dock menu and the status item
    ToggleWindow,
    StartAll,
    StopAll,
    // ---- the toolbar's search field
    /// The search field's text changed (or Return was pressed in it).
    Search,
    /// The search field's menu: filter the torrent names.
    SearchNames,
    /// The search field's menu: search this source (an index into the sources).
    SearchSource(u16),
    SearchAllSources,
    ManageSources,
    // slice D: the Files tab's filter and menu
    /// The Files tab's filter field changed (D50).
    FileFilter,
    /// Files: Open (also a double click or Return).
    FileOpen,
    /// Files: Open Containing Folder.
    FileOpenFolder,
    /// Files: Prioritize by File Order.
    FileByOrder,
    /// Files: a priority, an index into `detail::PRIORITIES` (the last is Don't Download).
    FilePriority(u16),
    /// Files: Relocate.
    FileRelocate,
    // slice C
    /// The status bar's update notice: opens the release page, the notice goes away.
    UpdateNotice,
    /// The status bar's network part: Preferences > Connection while the port is taken.
    NetworkStatus,
    // slice F
    /// A control of a torrent dialog (`dialogs::cmd`): the dialog's number and the control's
    /// action, which `dialogs::run` hands to that dialog.
    Dialog(u16),
    // slice E
    /// A control of the Preferences window: an index into its controls (`prefs`).
    Pref(u16),
}

/// The Files tab's commands (slice D), apart from [`ALL`], whose menus they are not in.
pub const FILES: [Command; 6] = [
    Command::FileFilter,
    Command::FileOpen,
    Command::FileOpenFolder,
    Command::FileByOrder,
    Command::FileRelocate,
    Command::FilePriority(0),
];

/// Every command: those without a value, then those with one (at index 0).
pub const ALL: [Command; 54] = [
    Command::About,
    Command::CheckUpdates,
    Command::Preferences,
    Command::SetupGuide,
    Command::Quit,
    Command::AddTorrent,
    Command::AddUrl,
    Command::CreateTorrent,
    Command::ImportQbt,
    Command::ImportUt,
    Command::NewProfile,
    Command::ProfileFolder,
    Command::ConnectRemote,
    Command::Find,
    Command::ToggleToolbar,
    Command::ToggleSidebar,
    Command::ToggleDetails,
    Command::ToggleStatusBar,
    Command::OpenFolder,
    Command::ForceStart,
    Command::Start,
    Command::Pause,
    Command::Stop,
    Command::QueueUp,
    Command::QueueDown,
    Command::QueueTop,
    Command::QueueBottom,
    Command::Remove,
    Command::RemoveAndDelete,
    Command::Recheck,
    Command::UpdateTracker,
    Command::SetLocation,
    Command::InitialSeeding,
    Command::NewLabel,
    Command::RemoveLabel,
    Command::Properties,
    Command::Rss,
    Command::Website,
    Command::Notices,
    Command::ToggleWindow,
    Command::StartAll,
    Command::StopAll,
    Command::Search,
    Command::SearchNames,
    Command::SearchAllSources,
    Command::ManageSources,
    Command::Profile(0),
    Command::DownLimit(0),
    Command::UpLimit(0),
    Command::Label(0),
    Command::AutoShutdown(0),
    Command::SearchSource(0),
    Command::Dialog(0),
    Command::Pref(0),
];

/// A tag's value part: 16 bits. Tags start at 1, so an untagged item (0) is no command.
const SHIFT: u32 = 16;

impl Command {
    /// `(kind, value)`: the kind numbers the variants from 1.
    fn code(self) -> (u16, u16) {
        use Command::*;
        match self {
            About => (1, 0),
            CheckUpdates => (2, 0),
            Preferences => (3, 0),
            SetupGuide => (4, 0),
            Quit => (5, 0),
            AddTorrent => (6, 0),
            AddUrl => (7, 0),
            CreateTorrent => (8, 0),
            ImportQbt => (9, 0),
            ImportUt => (10, 0),
            Profile(i) => (11, i),
            NewProfile => (12, 0),
            ProfileFolder => (13, 0),
            ConnectRemote => (14, 0),
            Find => (15, 0),
            ToggleToolbar => (16, 0),
            ToggleSidebar => (17, 0),
            ToggleDetails => (18, 0),
            ToggleStatusBar => (19, 0),
            OpenFolder => (20, 0),
            ForceStart => (21, 0),
            Start => (22, 0),
            Pause => (23, 0),
            Stop => (24, 0),
            QueueUp => (25, 0),
            QueueDown => (26, 0),
            QueueTop => (27, 0),
            QueueBottom => (28, 0),
            Remove => (29, 0),
            RemoveAndDelete => (30, 0),
            Recheck => (31, 0),
            UpdateTracker => (32, 0),
            SetLocation => (33, 0),
            InitialSeeding => (34, 0),
            DownLimit(i) => (35, i),
            UpLimit(i) => (36, i),
            NewLabel => (37, 0),
            RemoveLabel => (38, 0),
            Label(i) => (39, i),
            Properties => (40, 0),
            Rss => (41, 0),
            AutoShutdown(i) => (42, i),
            Website => (43, 0),
            Notices => (44, 0),
            ToggleWindow => (45, 0),
            StartAll => (46, 0),
            StopAll => (47, 0),
            Search => (48, 0),
            SearchNames => (49, 0),
            SearchSource(i) => (50, i),
            SearchAllSources => (51, 0),
            ManageSources => (52, 0),
            // slice D
            FileFilter => (53, 0),
            FileOpen => (54, 0),
            FileOpenFolder => (55, 0),
            FileByOrder => (56, 0),
            FilePriority(i) => (57, i),
            FileRelocate => (58, 0),
            // slice C (kinds from 100, apart from the other slices' additions)
            UpdateNotice => (100, 0),
            NetworkStatus => (101, 0),
            // slice F (kind 59: 53-58 are slice D's)
            Dialog(i) => (59, i),
            // slice E (kind 60: 100-101 are slice C's)
            Pref(i) => (60, i),
        }
    }

    fn from_code(kind: u16, v: u16) -> Option<Command> {
        use Command::*;
        let plain = |c: Command| (v == 0).then_some(c);
        match kind {
            1 => plain(About),
            2 => plain(CheckUpdates),
            3 => plain(Preferences),
            4 => plain(SetupGuide),
            5 => plain(Quit),
            6 => plain(AddTorrent),
            7 => plain(AddUrl),
            8 => plain(CreateTorrent),
            9 => plain(ImportQbt),
            10 => plain(ImportUt),
            11 => Some(Profile(v)),
            12 => plain(NewProfile),
            13 => plain(ProfileFolder),
            14 => plain(ConnectRemote),
            15 => plain(Find),
            16 => plain(ToggleToolbar),
            17 => plain(ToggleSidebar),
            18 => plain(ToggleDetails),
            19 => plain(ToggleStatusBar),
            20 => plain(OpenFolder),
            21 => plain(ForceStart),
            22 => plain(Start),
            23 => plain(Pause),
            24 => plain(Stop),
            25 => plain(QueueUp),
            26 => plain(QueueDown),
            27 => plain(QueueTop),
            28 => plain(QueueBottom),
            29 => plain(Remove),
            30 => plain(RemoveAndDelete),
            31 => plain(Recheck),
            32 => plain(UpdateTracker),
            33 => plain(SetLocation),
            34 => plain(InitialSeeding),
            35 => Some(DownLimit(v)),
            36 => Some(UpLimit(v)),
            37 => plain(NewLabel),
            38 => plain(RemoveLabel),
            39 => Some(Label(v)),
            40 => plain(Properties),
            41 => plain(Rss),
            42 => Some(AutoShutdown(v)),
            43 => plain(Website),
            44 => plain(Notices),
            45 => plain(ToggleWindow),
            46 => plain(StartAll),
            47 => plain(StopAll),
            48 => plain(Search),
            49 => plain(SearchNames),
            50 => Some(SearchSource(v)),
            51 => plain(SearchAllSources),
            52 => plain(ManageSources),
            // slice D
            53 => plain(FileFilter),
            54 => plain(FileOpen),
            55 => plain(FileOpenFolder),
            56 => plain(FileByOrder),
            57 => Some(FilePriority(v)),
            58 => plain(FileRelocate),
            // slice C
            100 => plain(UpdateNotice),
            101 => plain(NetworkStatus),
            // slice F
            59 => Some(Dialog(v)),
            // slice E
            60 => Some(Pref(v)),
            _ => None,
        }
    }

    /// The tag a menu item, toolbar item or control carries for this command (never 0).
    pub fn tag(self) -> isize {
        let (kind, v) = self.code();
        ((kind as isize) << SHIFT) | v as isize
    }

    /// The command a tag stands for; `None` for 0 and other tags that are none of ours.
    pub fn from_tag(tag: isize) -> Option<Command> {
        let tag = u32::try_from(tag).ok()?;
        Command::from_code(u16::try_from(tag >> SHIFT).ok()?, tag as u16)
    }

    /// Whether it acts on the torrents selected in the list (greyed without a selection).
    pub fn needs_selection(self) -> bool {
        use Command::*;
        matches!(
            self,
            OpenFolder
                | ForceStart
                | Start
                | Pause
                | Stop
                | QueueUp
                | QueueDown
                | QueueTop
                | QueueBottom
                | Remove
                | RemoveAndDelete
                | Recheck
                | UpdateTracker
                | SetLocation
                | InitialSeeding
                | DownLimit(_)
                | UpLimit(_)
                | NewLabel
                | RemoveLabel
                | Label(_)
                | Properties
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for c in ALL {
            assert_eq!(Command::from_tag(c.tag()), Some(c), "{c:?}");
            assert!(c.tag() > 0);
            seen.insert(c);
        }
        assert_eq!(seen.len(), ALL.len(), "a command is twice in ALL");
        for c in [Command::Label(7), Command::Profile(65535), Command::AutoShutdown(8), Command::UpLimit(9)] {
            assert_eq!(Command::from_tag(c.tag()), Some(c));
        }
        assert_eq!(Command::from_tag(0), None);
        assert_eq!(Command::from_tag(-1), None);
        assert_eq!(Command::from_tag(Command::Quit.tag() + 1), None, "a plain command takes no value");
        assert_eq!(Command::from_tag(61 << SHIFT), None);
    }

    #[test]
    fn files_tags_round_trip() {
        for c in FILES.into_iter().chain([Command::FilePriority(3)]) {
            assert_eq!(Command::from_tag(c.tag()), Some(c), "{c:?}");
            assert!(!ALL.contains(&c));
            assert!(!c.needs_selection(), "the Files tab's selection is not the list's");
        }
    }

    #[test]
    fn slice_c_tags_round_trip() {
        for c in [Command::UpdateNotice, Command::NetworkStatus] {
            assert_eq!(Command::from_tag(c.tag()), Some(c));
            assert!(!c.needs_selection());
        }
        assert_eq!(Command::from_tag(102 << SHIFT), None);
    }
}
