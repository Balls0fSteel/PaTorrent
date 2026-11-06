//! A test aid for the VM (as `prefs::tour`): with `ROOKPARCEL_SNAPSHOTS` naming a folder that
//! holds a file `rss-tour` whose first line is a feed's URL (a test script makes it; nothing else
//! reads it), opening the RSS Downloader starts a tour through slice H's parts, each step through
//! the controls' and commands' own actions: Add RSS Feed with that URL, the fetched feed, a
//! filter on "alpha" with a label and its preview, the History tab, closing the window (the
//! filter adds its item), the feed's items in the list, Download of the "beta" item, the Feeds
//! menu's items and Disable / Enable, Edit Feed and Delete Feed's question (both cancelled), the
//! torrents, and the History again. After each step it keeps the snapshot of the moment as
//! `<step>-window-<n>.png` and `<step>.txt`. Accessibility cannot reach the windows while the
//! VM's screen is locked; this can. Steps wait on a thread and come back through the main queue
//! (App Nap holds a background app's timers back on the locked VM).

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2_app_kit::NSAlertSecondButtonReturn;
use rookparcel_frontend::rss::FeedId;

use super::{MENU, feed_dialog, window};
use crate::app::{peek, ui};
use crate::commands::Command;
use crate::menu::Item;
use crate::ui::ns;

thread_local! {
    static STARTED: Cell<bool> = const { Cell::new(false) };
    static FEED: Cell<Option<FeedId>> = const { Cell::new(None) };
}

fn dir() -> Option<PathBuf> {
    let dir = std::env::var_os("ROOKPARCEL_SNAPSHOTS").filter(|d| !d.is_empty()).map(PathBuf::from)?;
    dir.join("rss-tour").exists().then_some(dir)
}

/// The window opened: the tour starts, once, when it is asked for.
pub(super) fn opened() {
    if dir().is_none() || STARTED.with(|s| s.replace(true)) {
        return;
    }
    eprintln!("rss tour: start");
    later(0, 1.0);
}

/// Runs step `step` `secs` seconds from now.
fn later(step: usize, secs: f64) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(secs));
        crate::mainq::on_main(move || match self::step(step) {
            Some(wait) => later(step + 1, wait),
            None => eprintln!("rss tour: done"),
        });
    });
}

/// Keeps the snapshot of now under `name`.
fn keep(name: &str) {
    let Some(dir) = dir() else { return };
    if let Some(mtm) = MainThreadMarker::new() {
        for w in objc2_app_kit::NSApplication::sharedApplication(mtm).windows().iter() {
            if let Some(v) = w.contentView() {
                v.layoutSubtreeIfNeeded();
            }
        }
    }
    crate::snapshots::take();
    for n in 1..=6 {
        let from = dir.join(format!("window-{n}.png"));
        let to = dir.join(format!("{name}-window-{n}.png"));
        if from.exists() {
            let _ = std::fs::copy(&from, to);
        }
    }
    let _ = std::fs::copy(dir.join("windows.txt"), dir.join(format!("{name}.txt")));
    eprintln!("rss tour: kept {name}");
}

fn url() -> String {
    let file = dir().map(|d| d.join("rss-tour")).unwrap_or_default();
    std::fs::read_to_string(file).unwrap_or_default().lines().next().unwrap_or_default().trim().to_owned()
}

/// A menu's items as text: title, enabled.
fn menu_text(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            Item::Cmd { cmd, title, .. } => format!("{title} [{}]", crate::app::state(*cmd).0),
            Item::Sep => "-".to_owned(),
            _ => "?".to_owned(),
        })
        .collect()
}

/// Runs a step; the seconds until the next one (`None`: the end).
fn step(n: usize) -> Option<f64> {
    eprintln!("rss tour: step {n}");
    match n {
        0 => {
            keep("rss-01-open");
            window::current()?.act(window::FEED_ADD);
            let d = feed_dialog::current()?;
            d.url.setStringValue(&ns(&url()));
            d.alias.setStringValue(&ns("Test feed"));
            Some(1.0)
        }
        1 => {
            keep("rss-02-add-feed");
            feed_dialog::current()?.act(feed_dialog::OK);
            Some(6.0)
        }
        2 => {
            let feeds = peek(|c| {
                c.rss()
                    .feeds
                    .iter()
                    .map(|f| (f.id, f.name().to_owned(), f.items.len(), f.last_error.clone()))
                    .collect::<Vec<_>>()
            })?;
            eprintln!("rss tour: feeds {feeds:?}");
            FEED.with(|f| f.set(feeds.first().map(|f| f.0)));
            keep("rss-03-fetched");
            let d = window::current()?;
            d.act(window::FILTER_ADD);
            d.name.setStringValue(&ns("Alpha only"));
            d.pattern.setStringValue(&ns("*alpha*"));
            d.label.setStringValue(&ns("rss-test"));
            d.act(window::STORE);
            Some(1.0)
        }
        3 => {
            let filters = peek(|c| c.rss().filters.clone())?;
            eprintln!("rss tour: filters {filters:?}");
            keep("rss-04-filter");
            window::current()?.tabs.selectTabViewItemAtIndex(1);
            Some(1.0)
        }
        4 => {
            keep("rss-05-history-before");
            let d = window::current()?;
            d.tabs.selectTabViewItemAtIndex(0);
            d.act(window::CLOSE);
            Some(6.0)
        }
        5 => {
            crate::sidebar::select_feed(FEED.with(Cell::get));
            Some(2.0)
        }
        6 => {
            keep("rss-06-feed-items");
            let row =
                peek(|c| c.feed_rows().iter().position(|r| r.cells[0].to_lowercase().contains("beta")))??;
            let items = super::ITEMS.with(|t| t.borrow().clone())?;
            items.view.selectRowIndexes_byExtendingSelection(&crate::table::index_set(vec![row]), false);
            eprintln!("rss tour: download state {:?}", crate::app::state(Command::FeedDownload));
            crate::app::run(Command::FeedDownload);
            Some(6.0)
        }
        7 => {
            keep("rss-07-after-download");
            let feed = FEED.with(Cell::get);
            MENU.with(|m| m.set((feed, false)));
            eprintln!("rss tour: feed menu {:?}", menu_text(&super::feed_menu(feed, true)));
            MENU.with(|m| m.set((None, false)));
            eprintln!("rss tour: Feeds row menu {:?}", menu_text(&super::feed_menu(None, true)));
            MENU.with(|m| m.set((feed, false)));
            crate::app::run(Command::FeedToggle);
            eprintln!("rss tour: after Disable {:?}", menu_text(&super::feed_menu(feed, true)));
            crate::app::run(Command::FeedToggle);
            eprintln!("rss tour: after Enable {:?}", menu_text(&super::feed_menu(feed, true)));
            crate::app::run(Command::FeedEdit);
            Some(1.0)
        }
        8 => {
            keep("rss-08-edit-feed");
            feed_dialog::current()?.act(feed_dialog::CANCEL);
            crate::app::run(Command::FeedDelete);
            Some(1.0)
        }
        9 => {
            keep("rss-09-delete-question");
            let main = ui()?.window.clone();
            if let Some(sheet) = main.attachedSheet() {
                main.endSheet_returnCode(&sheet, NSAlertSecondButtonReturn);
            }
            crate::sidebar::select_all_torrents();
            Some(3.0)
        }
        10 => {
            let torrents = peek(|c| {
                c.list()
                    .view()
                    .iter()
                    .filter_map(|&id| c.list().get(id).map(|s| s.name.clone()))
                    .collect::<Vec<_>>()
            })?;
            eprintln!("rss tour: torrents {torrents:?}, labels {:?}", crate::app::labels());
            eprintln!("rss tour: feeds now {:?}", super::sidebar_rows());
            keep("rss-10-torrents");
            crate::app::run(Command::Rss);
            window::current()?.tabs.selectTabViewItemAtIndex(1);
            Some(1.0)
        }
        11 => {
            keep("rss-11-history");
            None
        }
        _ => None,
    }
}
