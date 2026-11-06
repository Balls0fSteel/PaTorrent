//! RSS (slice H, docs/MACOS_GUI.md): the sidebar's Feeds rows and their context menu, a feed's
//! items in the main list, and the RSS Downloader window ([`window`]) with its Add RSS Feed
//! dialog ([`feed_dialog`]), on `frontend::app::Core`'s RSS part (GTK `rss.rs` and `app.rs`'s
//! RSS functions).
//!
//! - The sidebar lists every feed under Feeds ([`sidebar_rows`]); picking one ([`show_feed`])
//!   puts a table of its items (`rss::view::ITEM_COLUMNS`) in the torrent list's place, as GTK
//!   swaps its `feed_table` into `list_scroll`. A double click, Return or the context menu's
//!   Download adds the selected items ([`Command::FeedDownload`]); picking a torrent category
//!   brings the torrents back (`Core::show_torrents` → `Events::feeds` → [`on_events`]).
//! - A Feeds row's context menu ([`sidebar_menu_items`], AppKit asks when it opens) and the
//!   window's Feeds list's run the `Feed*` commands on the feed it was opened for.
//! - The window edits the live downloader (`Core::edit_rss`); closing it runs the filters over
//!   the feeds' items (`Core::rss_closed`).

mod feed_dialog;
mod tour;
mod window;

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSMenu, NSScrollView, NSWindow};
use rookparcel_frontend::lang;
use rookparcel_frontend::rss::FeedId;
use rookparcel_frontend::rss::view::{FeedForm, ITEM_COLUMNS};
use rookparcel_frontend::tr;

use crate::app::{apply, peek, ui, with_core};
use crate::commands::Command;
use crate::menu::{self, Item};
use crate::table::{self, Align, CellValue, ColumnSpec, Hooks, Key, RPTableSource, RPTableView, TableKind};
use crate::ui::{self as cui, layout};

/// The feed items' table in the main window (made the first time a feed is picked).
struct Items {
    scroll: Retained<NSScrollView>,
    view: Retained<RPTableView>,
    source: Retained<RPTableSource>,
}

thread_local! {
    static ITEMS: RefCell<Option<Rc<Items>>> = const { RefCell::new(None) };
    /// What the list showed at the last [`on_events`] (`Core::feed_view`).
    static SHOWN: Cell<Option<Option<FeedId>>> = const { Cell::new(None) };
    /// The feed a Feeds menu was opened for (`None`: the Feeds row, All Feeds), and whether it
    /// was the RSS Downloader window's (its dialogs go there).
    static MENU: Cell<(Option<FeedId>, bool)> = const { Cell::new((None, false)) };
}

/// Options > RSS Downloader and the toolbar's RSS button.
pub fn open() {
    window::show(None);
}

/// The sidebar's Feeds rows: each feed's id, and its name with the number of items its last
/// fetch brought ("Name (12)", as the categories show their counts).
pub fn sidebar_rows() -> Vec<(FeedId, String)> {
    peek(|c| c.rss().feeds.iter().map(|f| (f.id, format!("{} ({})", f.name(), f.items.len()))).collect())
        .unwrap_or_default()
}

/// The sidebar picked the Feeds row (`None`: every feed's items) or a feed: the list shows its
/// items.
pub fn show_feed(feed: Option<FeedId>) {
    apply(with_core(|c| {
        c.show_feed(Some(feed));
        c.events()
    }));
}

/// `Events::feeds`: the list shows the torrents or the feed items `Core` says, the items again,
/// and the open RSS Downloader window again.
pub fn on_events() {
    let view = peek(|c| c.feed_view()).flatten();
    sync_list(view);
    window::refresh();
}

/// The feed items' table, made and put beside the torrent list the first time.
fn items() -> Option<Rc<Items>> {
    if let Some(i) = ITEMS.with(|t| t.borrow().clone()) {
        return Some(i);
    }
    let mtm = MainThreadMarker::new()?;
    let u = ui()?;
    let specs: Vec<ColumnSpec> = ITEM_COLUMNS
        .iter()
        .enumerate()
        .map(|(i, &(title, width, right))| {
            let mut s =
                ColumnSpec::text(&format!("item{i}"), &cui::text(lang::text(title)), f64::from(width));
            s.hideable = i > 0;
            if right {
                s.align = Align::Right;
            }
            s
        })
        .collect();
    let mut hooks = Hooks::flat(
        || peek(|c| c.feed_rows().len()).unwrap_or(0),
        |row, i, _dark| {
            peek(|c| c.feed_rows().get(row as usize).and_then(|r| r.cells.get(i)).cloned())
                .flatten()
                .map_or(CellValue::Empty, CellValue::Text)
        },
    );
    // ids that survive a fetch moving the items: the feed and the item's id
    hooks.id = Box::new(|row| peek(|c| c.feed_rows().get(row).map(|r| item_key(r.feed, &r.id))).flatten());
    hooks.rows_of = Box::new(|ids| {
        peek(|c| {
            c.feed_rows()
                .iter()
                .enumerate()
                .filter(|(_, r)| ids.contains(&item_key(r.feed, &r.id)))
                .map(|(i, _)| i)
                .collect()
        })
        .unwrap_or_default()
    });
    hooks.activate = Some(Box::new(|_| download_selected()));
    hooks.key = Some(Box::new(|key, ids| {
        if key != Key::Return || ids.is_empty() {
            return false;
        }
        download_selected();
        true
    }));
    hooks.menu = Some(Box::new(|_ids| {
        let mtm = MainThreadMarker::new()?;
        let items = [
            Item::Cmd { cmd: Command::FeedDownload, title: cui::text(tr!(MenuDownload)), key: None },
            Item::Sep,
            Item::Cmd { cmd: Command::Rss, title: cui::text(tr!(MenuRss)), key: None },
        ];
        Some(menu::build("", &items, mtm))
    }));
    let (scroll, view, source) = table::table(TableKind("feed items"), specs, hooks, None, mtm);
    let alternate = peek(|c| c.gui_options().alternate_rows).unwrap_or(true);
    view.setUsesAlternatingRowBackgroundColors(alternate);
    scroll.setHidden(true);
    u.list_box.addSubview(&scroll);
    layout::pin(&scroll, &u.list_box, 0.0);
    let items = Rc::new(Items { scroll, view, source });
    ITEMS.with(|t| *t.borrow_mut() = Some(items.clone()));
    Some(items)
}

/// A feed item's id in the items' table.
fn item_key(feed: FeedId, id: &str) -> u64 {
    let mut h = DefaultHasher::new();
    (feed, id).hash(&mut h);
    h.finish()
}

/// Shows the feed items (`Some`) or the torrents in the list's place.
fn sync_list(view: Option<Option<FeedId>>) {
    let Some(u) = ui() else { return };
    let torrents = u.list_view.enclosingScrollView();
    let was = SHOWN.with(|s| s.replace(view));
    match view {
        Some(_) => {
            let Some(items) = items() else { return };
            if was != view {
                // SAFETY: a nil sender, as AppKit's own callers pass.
                unsafe { items.view.deselectAll(None) };
                items.source.reload_keep_selection();
                items.view.scrollRowToVisible(0);
            } else {
                items.source.reload_keep_selection();
            }
            items.scroll.setHidden(false);
            if let Some(t) = torrents {
                t.setHidden(true);
            }
        }
        None => {
            if let Some(t) = torrents {
                t.setHidden(false);
            }
            if let Some(items) = ITEMS.with(|t| t.borrow().clone()) {
                items.scroll.setHidden(true);
            }
        }
    }
}

/// The selected feed items' rows.
fn selected_rows() -> Vec<usize> {
    let Some(items) = ITEMS.with(|t| t.borrow().clone()) else { return Vec::new() };
    if items.scroll.isHidden() {
        return Vec::new();
    }
    table::rows_of_set(&items.view.selectedRowIndexes())
}

/// Download: the selected feed items are added (`Core::download_feed_items`).
fn download_selected() {
    let rows = selected_rows();
    if rows.is_empty() {
        return;
    }
    apply(with_core(|c| {
        c.download_feed_items(&rows);
        c.events()
    }));
}

/// A Feeds menu for `feed` (`None`: the Feeds row or All Feeds): Add RSS Feed, Edit Feed,
/// Disable / Enable Feed, Update Feed, Delete Feed, and (`rss`) RSS Downloader.
fn feed_menu(feed: Option<FeedId>, rss: bool) -> Vec<Item> {
    let enabled = peek(|c| c.feed_enabled(feed)).unwrap_or(true);
    let item = |cmd: Command, title: &str| Item::Cmd { cmd, title: cui::text(title), key: None };
    let mut items = vec![
        item(Command::FeedAdd, tr!(MenuAddFeed)),
        item(Command::FeedEdit, tr!(MenuEditFeed)),
        item(Command::FeedToggle, if enabled { tr!(MenuDisableFeed) } else { tr!(MenuEnableFeed) }),
        item(Command::FeedUpdate, tr!(MenuUpdateFeed)),
        item(Command::FeedDelete, tr!(MenuDeleteFeed)),
    ];
    if rss {
        items.push(Item::Sep);
        items.push(item(Command::FeedRss, tr!(MenuRss)));
    }
    items
}

/// The sidebar's context menu as it opens (`menuNeedsUpdate:`): the Feeds menu on the Feeds
/// row and the feeds' rows, nothing (no menu) on the others.
pub fn sidebar_menu_items() -> Vec<Item> {
    match crate::sidebar::clicked_feed() {
        Some(feed) => {
            MENU.with(|m| m.set((feed, false)));
            feed_menu(feed, true)
        }
        None => Vec::new(),
    }
}

/// The RSS Downloader's Feeds list's context menu, for `feed`.
fn window_menu(feed: Option<FeedId>, mtm: MainThreadMarker) -> Retained<NSMenu> {
    MENU.with(|m| m.set((feed, true)));
    menu::build("", &feed_menu(feed, false), mtm)
}

/// Whether a feed command is enabled (`None` for the other commands).
pub fn state(cmd: Command) -> Option<(bool, Option<bool>)> {
    let feed = MENU.with(Cell::get).0;
    let on = match cmd {
        Command::FeedAdd | Command::FeedUpdate | Command::FeedRss => true,
        Command::FeedEdit | Command::FeedToggle | Command::FeedDelete => {
            feed.is_some_and(|id| peek(|c| c.feed_name(id).is_some()).unwrap_or(false))
        }
        Command::FeedDownload => !selected_rows().is_empty(),
        _ => return None,
    };
    Some((on, None))
}

/// Where a feed command's dialogs go: the RSS Downloader when its menu ran it, else the main
/// window.
fn parent() -> Option<Retained<NSWindow>> {
    let from_window = MENU.with(Cell::get).1;
    from_window.then(window::ns_window).flatten().or_else(crate::dialogs::main_window)
}

/// Runs a feed command on the feed its menu was opened for.
pub fn run(cmd: Command) {
    let feed = MENU.with(Cell::get).0;
    match cmd {
        Command::FeedAdd => add_feed(parent()),
        Command::FeedEdit => {
            if let Some(id) = feed {
                edit_feed(parent(), id);
            }
        }
        Command::FeedToggle => {
            if let Some(id) = feed {
                with_core(|c| c.toggle_feed(id));
                window::refresh();
            }
        }
        Command::FeedUpdate => update_feed(feed),
        Command::FeedDelete => {
            if let Some(id) = feed {
                delete_feed(parent(), id);
            }
        }
        Command::FeedRss => window::show(feed),
        Command::FeedDownload => download_selected(),
        _ => {}
    }
}

/// Add RSS Feed over `parent`: the feed is added and fetched; the open window picks it.
fn add_feed(parent: Option<Retained<NSWindow>>) {
    feed_dialog::add(parent, |form| {
        let Some((id, ev)) = with_core(|c| {
            let id = c.add_feed(&form);
            (id, c.events())
        }) else {
            return;
        };
        apply(Some(ev));
        window::feed_added(id);
    });
}

/// Edit Feed over `parent`.
fn edit_feed(parent: Option<Retained<NSWindow>>, id: FeedId) {
    let Some(form) = peek(|c| c.rss().feed(id).map(FeedForm::of)).flatten() else { return };
    feed_dialog::show(parent, tr!(RssEditFeed), form, move |form| {
        apply(with_core(|c| {
            c.edit_feed(id, &form);
            c.events()
        }));
    });
}

/// Update Feed (`None`: every enabled feed).
fn update_feed(feed: Option<FeedId>) {
    with_core(|c| c.update_feed(feed));
    window::refresh();
}

/// Delete Feed, after its question (a sheet on `parent`).
fn delete_feed(parent: Option<Retained<NSWindow>>, id: FeedId) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let name = peek(|c| c.feed_name(id)).flatten().unwrap_or_default();
    cui::sheet(
        parent.as_deref(),
        cui::Style::Warning,
        &tr!(AskDeleteFeed, name),
        "",
        &[tr!(BtnOk), tr!(BtnCancel)],
        move |button| {
            if button == 0 {
                apply(with_core(|c| {
                    c.remove_feed(id);
                    c.events()
                }));
            }
        },
        mtm,
    );
}
