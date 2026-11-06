//! Torrent search through the user's Prowlarr or Jackett over Torznab (D41, slice I of
//! docs/MACOS_GUI.md), on `frontend::search` as the GTK and Win32 windows:
//!
//! - The toolbar's search field filters the torrent names; its menu (`searchMenuTemplate`,
//!   [`menu_items`]) picks what it does instead: "Search torrent names", one source, "All
//!   sources", and "Manage Sources...". The mode is kept as `ui.search_source`
//!   (`view::Mode`), its grey hint says what Enter does ([`set_mode`]). In a source mode the field
//!   sends its text on Return only, and Return opens Search Results ([`run`]).
//! - [`results`]: the Search Results window (one, not modal).
//! - [`sources`]: the Search Sources window and the Add / Edit Search Source sheet; the search
//!   helper's buttons are `crate::search_helper`'s.
//!
//! The sources are `Core`'s preferences (`search.source.N.*`); [`store`] keeps an edit and
//! updates the field's menu, its mode and the open windows.

pub mod results;
pub mod sources;
mod tour;

use std::cell::RefCell;

use objc2::MainThreadMarker;
use objc2_app_kit::NSSearchFieldCell;
use rookparcel_frontend::search::view::{Mode, mode_menu};
use rookparcel_frontend::search::{Source, helper};
use rookparcel_frontend::tr;

use crate::app::{apply, peek, ui, with_core};
use crate::commands::Command;
use crate::menu::{self, Item};
use crate::ui::{self as rp, ns};

thread_local! {
    /// The search field's mode, read from the prefs at the first call.
    static MODE: RefCell<Option<Mode>> = const { RefCell::new(None) };
}

/// The stored sources (keys included: never show them).
pub fn stored_sources() -> Vec<Source> {
    peek(|c| c.search_sources()).unwrap_or_default()
}

/// The search field's mode.
pub fn mode() -> Mode {
    if let Some(m) = MODE.with(|m| m.borrow().clone()) {
        return m;
    }
    let Some(m) = peek(|c| Mode::load(c.prefs()).resolve(&c.search_sources())) else { return Mode::Filter };
    MODE.with(|x| *x.borrow_mut() = Some(m.clone()));
    m
}

/// Whether the search field filters the torrent names (the default mode, checked in its menu);
/// `false` while it searches a source.
pub fn filters_names() -> bool {
    mode().is_filter()
}

/// The search field menu's items: the modes ([`mode_menu`]), then Manage Sources.
pub fn menu_items() -> Vec<Item> {
    let sources = stored_sources();
    let mut items: Vec<Item> = mode_menu(&sources, &mode())
        .into_iter()
        .map(|e| {
            let (cmd, title) = match (e.index, &e.mode) {
                // a source's name is the user's text, shown as it is
                (Some(i), _) => (Command::SearchSource(i as u16), e.text),
                (None, Mode::All) => (Command::SearchAllSources, rp::text(&e.text)),
                (None, _) => (Command::SearchNames, rp::text(&e.text)),
            };
            Item::Cmd { cmd, title, key: None }
        })
        .collect();
    items.push(Item::Sep);
    items.push(Item::Cmd { cmd: Command::ManageSources, title: rp::text(tr!(SearchManage)), key: None });
    items
}

/// The search field's grey hint for the mode now.
pub fn cue() -> String {
    mode().cue(&stored_sources())
}

/// The mode items' state for `app::state`: the current one checked, All sources greyed with no
/// source switched on.
pub fn state(cmd: Command) -> (bool, Option<bool>) {
    let m = mode();
    match cmd {
        Command::SearchNames => (true, Some(m.is_filter())),
        Command::SearchAllSources => (stored_sources().iter().any(|s| s.enabled), Some(m == Mode::All)),
        Command::SearchSource(i) => (true, Some(Mode::of_source(&stored_sources(), i as usize) == Some(m))),
        _ => (true, None),
    }
}

/// The search field menu's items (the modes).
pub fn set_mode(cmd: Command) {
    let sources = stored_sources();
    let m = match cmd {
        Command::SearchNames => Mode::Filter,
        Command::SearchAllSources => Mode::All,
        Command::SearchSource(i) => Mode::of_source(&sources, i as usize).unwrap_or_default(),
        _ => return,
    };
    use_mode(m.resolve(&sources), true);
}

/// Makes `m` the mode: kept in the prefs, the field's hint, menu and sending, the list's name
/// filter (only the filter mode filters); `focus` gives the field the keyboard.
fn use_mode(m: Mode, focus: bool) {
    MODE.with(|x| *x.borrow_mut() = Some(m.clone()));
    with_core(|c| {
        m.store(c.prefs_mut());
        c.save_prefs();
    });
    let Some(u) = ui() else { return };
    let filter = m.is_filter();
    u.search.setPlaceholderString(Some(&ns(&cue())));
    // a query goes on Return; the name filter as it is typed
    u.search.setSendsWholeSearchString(!filter);
    let text = if filter { rp::value(&u.search) } else { String::new() };
    apply(with_core(|c| {
        c.set_search(&text);
        c.events()
    }));
    refresh_menu();
    if focus {
        u.window.makeFirstResponder(Some(&u.search));
    }
}

/// Builds the search field's menu again (the sources changed).
pub fn refresh_menu() {
    let (Some(u), Some(mtm)) = (ui(), MainThreadMarker::new()) else { return };
    let template = menu::build("", &menu_items(), mtm);
    if let Some(cell) = u.search.cell().and_then(|c| c.downcast::<NSSearchFieldCell>().ok()) {
        cell.setSearchMenuTemplate(Some(&template));
    }
}

/// The search field's text in a source mode (Return): Search Results searches it.
pub fn run(text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let m = mode();
    if !m.is_filter() {
        results::search(text, &m);
    }
}

/// Once the main window is up (`app::start`): the kept mode's hint and menu; the VM's test tour
/// when asked for.
pub fn started() {
    use_mode(mode(), false);
    tour::start();
}

/// Stores `sources` (an edit in the Search Sources window): the field's menu and mode, and the
/// Search Results window's source list follow.
pub fn store(sources: &[Source]) {
    with_core(|c| c.store_search_sources(sources));
    let now = mode().resolve(sources);
    use_mode(now, false);
    results::sources_changed();
}

/// Tells the search helper (D51) where the profile's data folder is and which proxy to give it;
/// whether this GUI offers the helper.
pub fn configure_helper() -> bool {
    let Some((dir, proxy)) = peek(|c| (c.dirs().data.clone(), c.helper_proxy())) else { return false };
    helper::configure(&dir, proxy);
    helper::available()
}

/// At quit (`app::shutdown`): the search stops, the helper (if it runs) is killed.
pub fn shutdown() {
    results::stop();
    helper::shutdown();
}
