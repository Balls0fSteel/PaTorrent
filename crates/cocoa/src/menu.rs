//! The menus (docs/MACOS_GUI.md "Menus and shortcuts"): the main menu bar, the Dock menu, the
//! status item's menu and the Torrent menu, which is also the list's context menu.
//!
//! The trees are plain data ([`tree`], [`dock`], [`status_item`], [`torrent`]), built and tested
//! on every system; [`build_main`] and [`build`] turn them into `NSMenu`s on macOS. Our items run
//! a [`Command`] through `RPTarget`; AppKit's own (Hide, Minimize, Cut, Full Screen, ...) send
//! their standard action to the first responder (nil target), so they work wherever the focus
//! is: Edit's items in the search field, Minimize on any window. The lists that change (File >
//! Profiles, Torrent > Label) are rebuilt each time they open ([`needs_update`]).

use std::ffi::CStr;

use rookparcel_frontend::actions::{LIMITS, limit_text};
use rookparcel_frontend::auto_shutdown::Mode;
use rookparcel_frontend::tr;

use crate::commands::Command;
use crate::ui::text;

/// A key equivalent: the key (lower case, or a special character) and its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub key: &'static str,
    pub shift: bool,
    pub option: bool,
    pub control: bool,
}

/// ⌘ and a key.
const fn cmd(key: &'static str) -> Option<Key> {
    Some(Key { key, shift: false, option: false, control: false })
}

const fn shift_cmd(key: &'static str) -> Option<Key> {
    Some(Key { key, shift: true, option: false, control: false })
}

const fn option_cmd(key: &'static str) -> Option<Key> {
    Some(Key { key, shift: false, option: true, control: false })
}

const fn control_cmd(key: &'static str) -> Option<Key> {
    Some(Key { key, shift: false, option: false, control: true })
}

/// The ⌫ key as a key equivalent (`NSBackspaceCharacter`).
pub const BACKSPACE: &str = "\u{8}";

/// What a submenu is to AppKit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Plain,
    /// The application menu (its title is the bundle's name, whatever is set).
    App,
    /// Services ▸: AppKit fills it.
    Services,
    /// Window: AppKit lists the windows in it.
    Window,
    /// Help: AppKit adds its search field.
    Help,
    /// File > Profiles: rebuilt when it opens.
    Profiles,
    /// Torrent > Label: rebuilt when it opens.
    Labels,
}

/// A menu item.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// Runs a command.
    Cmd {
        cmd: Command,
        title: String,
        key: Option<Key>,
    },
    /// AppKit's standard action, sent to the first responder.
    Sys {
        action: &'static CStr,
        title: String,
        key: Option<Key>,
    },
    Sub {
        title: String,
        items: Vec<Item>,
        role: Role,
    },
    Sep,
}

fn c(cmd: Command, title: &str, key: Option<Key>) -> Item {
    Item::Cmd { cmd, title: text(title), key }
}

fn sys(action: &'static CStr, title: &str, key: Option<Key>) -> Item {
    Item::Sys { action, title: text(title), key }
}

fn sub(title: &str, items: Vec<Item>, role: Role) -> Item {
    Item::Sub { title: text(title), items, role }
}

/// The main menu bar: the application menu, File, Edit, View, Torrent, Options, Window, Help.
pub fn tree() -> Vec<Item> {
    use Command::*;
    let app = vec![
        c(About, tr!(MenuAbout), None),
        c(CheckUpdates, tr!(MenuCheckUpdates), None),
        Item::Sep,
        c(Preferences, tr!(MenuPreferences), cmd(",")),
        c(SetupGuide, tr!(MenuSetupGuide), cmd("g")),
        Item::Sep,
        sub(tr!(MacServices), Vec::new(), Role::Services),
        Item::Sep,
        sys(c"hide:", tr!(MenuHide), cmd("h")),
        sys(c"hideOtherApplications:", tr!(MacHideOthers), option_cmd("h")),
        sys(c"unhideAllApplications:", tr!(MacShowAll), None),
        Item::Sep,
        c(Quit, tr!(MacQuit), cmd("q")),
    ];
    let file = vec![
        c(AddTorrent, tr!(MenuAddTorrent), cmd("o")),
        c(AddUrl, tr!(MenuAddUrl), cmd("u")),
        c(CreateTorrent, tr!(MenuCreate), cmd("n")),
        Item::Sep,
        c(ImportQbt, tr!(MenuImportQbt), None),
        c(ImportUt, tr!(MenuImportUt), None),
        Item::Sep,
        sub(tr!(MenuProfiles), profiles(&[]), Role::Profiles),
        c(ConnectRemote, tr!(MenuConnectRemote), None),
        Item::Sep,
        sys(c"performClose:", tr!(MacCloseWindow), cmd("w")),
    ];
    let edit = vec![
        sys(c"undo:", tr!(MacUndo), cmd("z")),
        sys(c"redo:", tr!(MacRedo), shift_cmd("z")),
        Item::Sep,
        sys(c"cut:", tr!(MacCut), cmd("x")),
        sys(c"copy:", tr!(MacCopy), cmd("c")),
        sys(c"paste:", tr!(MacPaste), cmd("v")),
        sys(c"delete:", tr!(MacDelete), None),
        sys(c"selectAll:", tr!(MenuSelectAll), cmd("a")),
        Item::Sep,
        c(Find, tr!(MacFind), cmd("f")),
    ];
    let view = vec![
        c(ToggleToolbar, tr!(MenuShowToolbar), option_cmd("t")),
        sys(c"runToolbarCustomizationPalette:", tr!(MacCustomizeToolbar), None),
        Item::Sep,
        c(ToggleSidebar, tr!(MacShowSidebar), control_cmd("s")),
        c(ToggleDetails, tr!(MenuShowDetails), None),
        c(ToggleStatusBar, tr!(MenuShowStatus), None),
        Item::Sep,
        sys(c"toggleFullScreen:", tr!(MacEnterFullScreen), control_cmd("f")),
    ];
    let options =
        vec![c(Rss, tr!(MenuRss), None), Item::Sep, sub(tr!(MenuAutoShutdown), auto_shutdown(), Role::Plain)];
    let window = vec![
        sys(c"performMiniaturize:", tr!(MacMinimize), cmd("m")),
        sys(c"performZoom:", tr!(MacZoom), None),
        Item::Sep,
        sys(c"arrangeInFront:", tr!(MacBringAllToFront), None),
    ];
    let help = vec![c(Website, tr!(MenuWebsite), None), c(Notices, tr!(MenuNotices), None)];
    vec![
        sub("Rookparcel", app, Role::App),
        sub(tr!(MenuFile), file, Role::Plain),
        sub(tr!(MacMenuEdit), edit, Role::Plain),
        sub(tr!(MacMenuView), view, Role::Plain),
        sub(tr!(MacMenuTorrent), torrent(&[], true), Role::Plain),
        sub(tr!(MenuOptions), options, Role::Plain),
        sub(tr!(MacMenuWindow), window, Role::Window),
        sub(tr!(MenuHelp), help, Role::Help),
    ]
}

/// The Torrent menu, and the list's context menu (`shortcuts` off: a context menu shows none).
/// `labels` are the label submenu's names.
pub fn torrent(labels: &[String], shortcuts: bool) -> Vec<Item> {
    use Command::*;
    let k = |key: Option<Key>| if shortcuts { key } else { None };
    let limits = |make: fn(u16) -> Command| -> Vec<Item> {
        LIMITS.iter().enumerate().map(|(i, &kib)| c(make(i as u16), &limit_text(kib), None)).collect()
    };
    vec![
        c(OpenFolder, tr!(MenuOpenFolder), None),
        Item::Sep,
        c(ForceStart, tr!(MenuForceStart), None),
        c(Start, tr!(MenuStart), k(cmd("s"))),
        c(Pause, tr!(MenuPause), k(shift_cmd("p"))),
        c(Stop, tr!(MenuStop), k(cmd("t"))),
        Item::Sep,
        c(QueueUp, tr!(MenuQueueUp), None),
        c(QueueDown, tr!(MenuQueueDown), None),
        c(QueueTop, tr!(MenuQueueTop), None),
        c(QueueBottom, tr!(MenuQueueBottom), None),
        Item::Sep,
        c(Remove, tr!(MenuRemove), k(cmd(BACKSPACE))),
        c(RemoveAndDelete, tr!(MenuRemoveAndDelete), k(option_cmd(BACKSPACE))),
        Item::Sep,
        c(Recheck, tr!(MenuRecheck), None),
        c(UpdateTracker, tr!(MenuUpdateTracker), None),
        Item::Sep,
        sub(
            tr!(MenuAdvanced),
            vec![
                c(SetLocation, tr!(MenuSetLocation), None),
                c(InitialSeeding, tr!(MenuInitialSeeding), None),
            ],
            Role::Plain,
        ),
        sub(
            tr!(MenuBandwidth),
            vec![
                sub(tr!(MenuSetDownLimit), limits(DownLimit), Role::Plain),
                sub(tr!(MenuSetUpLimit), limits(UpLimit), Role::Plain),
            ],
            Role::Plain,
        ),
        sub(tr!(MenuLabel), label_items(labels), Role::Labels),
        Item::Sep,
        c(Properties, tr!(MenuProperties), k(cmd("i"))),
    ]
}

/// Torrent > Label's items: New Label, Remove Label, then the labels.
pub fn label_items(labels: &[String]) -> Vec<Item> {
    let mut items = vec![
        c(Command::NewLabel, tr!(MenuNewLabel), None),
        c(Command::RemoveLabel, tr!(MenuRemoveLabel), None),
    ];
    if !labels.is_empty() {
        items.push(Item::Sep);
    }
    // a label is the user's text, shown as it is (no `&` handling)
    items.extend(labels.iter().enumerate().map(|(i, l)| Item::Cmd {
        cmd: Command::Label(i as u16),
        title: l.clone(),
        key: None,
    }));
    items
}

/// File > Profiles' items: each profile (`None`: the default one), then New Profile and Open
/// Profile Folder.
pub fn profiles(names: &[Option<String>]) -> Vec<Item> {
    let mut items: Vec<Item> = names
        .iter()
        .enumerate()
        .map(|(i, n)| match n {
            Some(n) => Item::Cmd { cmd: Command::Profile(i as u16), title: n.clone(), key: None },
            None => c(Command::Profile(i as u16), tr!(MenuProfileDefault), None),
        })
        .collect();
    if !items.is_empty() {
        items.push(Item::Sep);
    }
    items.push(c(Command::NewProfile, tr!(MenuProfileNew), None));
    items.push(c(Command::ProfileFolder, tr!(MenuProfileFolder), None));
    items
}

/// Options > Auto Shutdown's items, in `Mode::MENU` order.
fn auto_shutdown() -> Vec<Item> {
    let mut items = Vec::new();
    for (i, m) in Mode::MENU.iter().enumerate() {
        items.push(c(Command::AutoShutdown(i as u16), Mode::label(*m), None));
        // Disabled stands apart from the modes
        if i == 0 {
            items.push(Item::Sep);
        }
    }
    items
}

/// The Dock menu (AppKit adds the windows, Options and Quit).
pub fn dock(window_visible: bool) -> Vec<Item> {
    use Command::*;
    vec![
        c(ToggleWindow, if window_visible { tr!(MenuHide) } else { tr!(MenuShow) }, None),
        Item::Sep,
        c(AddTorrent, tr!(MenuAddTorrent), None),
        c(AddUrl, tr!(MenuAddUrl), None),
        Item::Sep,
        c(StartAll, tr!(MenuStartAll), None),
        c(StopAll, tr!(MenuStopAll), None),
    ]
}

/// The status item's menu: the Dock menu's, and Quit.
pub fn status_item(window_visible: bool) -> Vec<Item> {
    let mut items = dock(window_visible);
    items.push(Item::Sep);
    items.push(c(Command::Quit, tr!(MacQuit), None));
    items
}

/// Every command a tree's items run, with their keys.
pub fn commands(items: &[Item], out: &mut Vec<(Command, Option<Key>)>) {
    for i in items {
        match i {
            Item::Cmd { cmd, key, .. } => out.push((*cmd, *key)),
            Item::Sub { items, .. } => commands(items, out),
            Item::Sys { .. } | Item::Sep => {}
        }
    }
}

#[cfg(target_os = "macos")]
pub use sys::*;

#[cfg(target_os = "macos")]
mod sys {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2::runtime::{ProtocolObject, Sel};
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
    use objc2_foundation::NSString;

    use super::{Item, Key, Role};
    use crate::ui::target::{target, wire};

    thread_local! {
        /// The menus rebuilt when they open, with what they are.
        static LIVE: RefCell<Vec<(Retained<NSMenu>, Role)>> = const { RefCell::new(Vec::new()) };
    }

    fn mask(k: &Key) -> NSEventModifierFlags {
        let mut m = NSEventModifierFlags::Command;
        if k.shift {
            m |= NSEventModifierFlags::Shift;
        }
        if k.option {
            m |= NSEventModifierFlags::Option;
        }
        if k.control {
            m |= NSEventModifierFlags::Control;
        }
        m
    }

    fn new_item(
        title: &str,
        action: Option<Sel>,
        key: Option<&Key>,
        mtm: MainThreadMarker,
    ) -> Retained<NSMenuItem> {
        let k = NSString::from_str(key.map_or("", |k| k.key));
        // SAFETY: `action` is a valid selector (ours or AppKit's standard one) or none.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                action,
                &k,
            )
        };
        if let Some(k) = key {
            item.setKeyEquivalentModifierMask(mask(k));
        }
        item
    }

    /// A menu of `items`.
    pub fn build(title: &str, items: &[Item], mtm: MainThreadMarker) -> Retained<NSMenu> {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
        fill(&menu, items, mtm);
        menu
    }

    /// Replaces `menu`'s items with `items`.
    pub fn fill(menu: &NSMenu, items: &[Item], mtm: MainThreadMarker) {
        menu.removeAllItems();
        for i in items {
            menu.addItem(&make(i, mtm));
        }
    }

    fn make(item: &Item, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
        match item {
            Item::Sep => NSMenuItem::separatorItem(mtm),
            Item::Cmd { cmd, title, key } => {
                let i = new_item(title, None, key.as_ref(), mtm);
                wire(&i, *cmd, mtm);
                i
            }
            Item::Sys { action, title, key } => {
                new_item(title, Some(Sel::register(action)), key.as_ref(), mtm)
            }
            Item::Sub { title, items, role } => {
                let i = new_item(title, None, None, mtm);
                let sub = build(title, items, mtm);
                if matches!(role, Role::Profiles | Role::Labels) {
                    sub.setDelegate(Some(ProtocolObject::from_ref(&*target(mtm))));
                    LIVE.with(|l| l.borrow_mut().push((sub.clone(), *role)));
                }
                let app = NSApplication::sharedApplication(mtm);
                match role {
                    Role::Services => app.setServicesMenu(Some(&sub)),
                    Role::Window => app.setWindowsMenu(Some(&sub)),
                    Role::Help => app.setHelpMenu(Some(&sub)),
                    _ => {}
                }
                i.setSubmenu(Some(&sub));
                i
            }
        }
    }

    /// Sets the application's main menu from [`super::tree`].
    pub fn build_main(mtm: MainThreadMarker) {
        let bar = build("", &super::tree(), mtm);
        NSApplication::sharedApplication(mtm).setMainMenu(Some(&bar));
    }

    /// `menuNeedsUpdate:` from `RPTarget`: rebuilds a live list (profiles, labels).
    pub fn needs_update(menu: &NSMenu) {
        let Some(role) =
            LIVE.with(|l| l.borrow().iter().find(|(m, _)| std::ptr::eq(&**m, menu)).map(|(_, r)| *r))
        else {
            return;
        };
        let mtm = MainThreadMarker::from(menu);
        let items = match role {
            Role::Profiles => super::profiles(&crate::app::profile_names()),
            Role::Labels => super::label_items(&crate::app::labels()),
            _ => return,
        };
        fill(menu, &items, mtm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_somewhere_and_keys_are_unique() {
        let mut all = Vec::new();
        commands(&tree(), &mut all);
        commands(&status_item(true), &mut all);
        let have: std::collections::HashSet<Command> = all.iter().map(|(c, _)| *c).collect();
        // the search field's own, and the lists filled at run time
        let elsewhere = |c: Command| {
            matches!(
                c,
                Command::Search
                    | Command::SearchNames
                    | Command::SearchSource(_)
                    | Command::SearchAllSources
                    | Command::ManageSources
                    | Command::Label(_)
                    | Command::Profile(_)
                    // slice G: the status item's button, the closing window's and the countdown's
                    | Command::StatusItem
                    | Command::QuitNow
                    | Command::CancelShutdown
            )
        };
        for c in crate::commands::ALL {
            assert!(have.contains(&c) || elsewhere(c), "{c:?} is in no menu");
        }
        let mut keys = Vec::new();
        commands(&tree(), &mut keys);
        let mut seen = std::collections::HashSet::new();
        for (c, k) in keys.into_iter().filter_map(|(c, k)| k.map(|k| (c, k))) {
            assert!(seen.insert(k), "{c:?}: {k:?} is taken twice");
        }
        // the context menu shows no shortcuts
        let mut ctx = Vec::new();
        commands(&torrent(&["a".into()], false), &mut ctx);
        assert!(ctx.iter().all(|(_, k)| k.is_none()));
        assert!(ctx.contains(&(Command::Label(0), None)));
    }

    #[test]
    fn no_mnemonics_in_titles() {
        fn walk(items: &[Item]) {
            for i in items {
                match i {
                    Item::Cmd { title, .. } | Item::Sys { title, .. } => {
                        assert!(!title.contains('&') && !title.ends_with("..."), "{title}")
                    }
                    Item::Sub { title, items, .. } => {
                        assert!(!title.contains('&'), "{title}");
                        walk(items);
                    }
                    Item::Sep => {}
                }
            }
        }
        walk(&tree());
        walk(&dock(false));
    }
}
