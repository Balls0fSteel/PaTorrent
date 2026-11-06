//! A test aid for the VM (as `prefs::tour`): with `ROOKPARCEL_SNAPSHOTS` naming a folder in
//! which a test script writes a file `search-tour`, the tour stores that file's search sources
//! and walks the search windows through their own code paths, the way clicks would: Search
//! Sources (a Test, the Add Search Source sheet with a bad URL, then a good one), the search
//! field's mode (its menu and hint logged), a search in Search Results, a sort, adding the first
//! result, a search of all sources, adding it again, and adding a result whose link is no
//! torrent. Each step waits for what it started (an answer, a fetch), not for a timer (App Nap
//! holds a background app's timers back on the locked VM). After each
//! step it keeps the snapshot of the moment as `search-<step>-window-<n>.png` and
//! `search-<step>.txt`. Nothing reads the file otherwise.
//!
//! The file: one source per line, `name|Torznab URL|API key`; a line `query=TEXT` sets the query
//! (default `test`); a line `bad=URL` the URL of the second source added through the sheet.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::Duration;

use objc2_app_kit::NSTableView;
use objc2_foundation::{NSArray, NSIndexSet, NSSortDescriptor};
use rookparcel_frontend::search::Source;

use super::{menu_items, mode, results, sources};
use crate::commands::Command;
use crate::menu::Item;
use crate::ui::ns;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Off,
    Testing,
    Searching,
    Adding,
    SearchingAll,
    AddingAgain,
    AddingBroken,
    Done,
}

thread_local! {
    static STEP: Cell<Step> = const { Cell::new(Step::Off) };
    static QUERY: RefCell<String> = RefCell::new(String::from("test"));
    static BAD: RefCell<String> = const { RefCell::new(String::new()) };
}

fn dir() -> Option<PathBuf> {
    let dir = std::env::var_os("ROOKPARCEL_SNAPSHOTS").filter(|d| !d.is_empty()).map(PathBuf::from)?;
    dir.join("search-tour").exists().then_some(dir)
}

fn step() -> Step {
    STEP.with(Cell::get)
}

fn set(s: Step) {
    STEP.with(|x| x.set(s));
    eprintln!("search tour: {s:?}");
}

/// Keeps the snapshot of now under `search-<name>`.
fn keep(name: &str) {
    let Some(dir) = dir() else { return };
    crate::snapshots::take();
    for n in 1..=6 {
        let from = dir.join(format!("window-{n}.png"));
        if from.exists() {
            let _ = std::fs::copy(&from, dir.join(format!("search-{name}-window-{n}.png")));
        }
    }
    let _ = std::fs::copy(dir.join("windows.txt"), dir.join(format!("search-{name}.txt")));
    eprintln!("search tour: kept {name}");
}

/// Logs the search field's menu (titles, check marks, greyed) and its hint.
fn log_field() {
    let items: Vec<String> = menu_items()
        .iter()
        .map(|i| match i {
            Item::Cmd { cmd, title, .. } => {
                let (on, checked) =
                    if *cmd == Command::ManageSources { (true, None) } else { super::state(*cmd) };
                format!(
                    "{}{}{}",
                    if checked == Some(true) { "✓ " } else { "" },
                    title,
                    if on { "" } else { " (greyed)" }
                )
            }
            _ => "—".to_owned(),
        })
        .collect();
    let hint = crate::app::ui()
        .and_then(|u| u.search.placeholderString())
        .map(|s| s.to_string())
        .unwrap_or_default();
    eprintln!("search tour: mode {:?}, field menu {items:?}, hint {hint:?}", mode());
}

/// From `search::started`: with the snapshots on, waits on a thread (five minutes at most) for a
/// test script to write `search-tour` (`run.sh` starts with an empty folder), then begins.
pub fn start() {
    let Some(snapshots) =
        std::env::var_os("ROOKPARCEL_SNAPSHOTS").filter(|d| !d.is_empty()).map(PathBuf::from)
    else {
        return;
    };
    std::thread::spawn(move || {
        for _ in 0..300 {
            if snapshots.join("search-tour").exists() {
                // written whole
                std::thread::sleep(Duration::from_millis(500));
                crate::mainq::on_main(begin);
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// The tour's first steps: the sources stored, Search Sources and a Test.
fn begin() {
    let Some(dir) = dir() else { return };
    if step() != Step::Off {
        return;
    }
    let text = std::fs::read_to_string(dir.join("search-tour")).unwrap_or_default();
    let mut list = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(q) = line.strip_prefix("query=") {
            QUERY.with(|x| *x.borrow_mut() = q.to_owned());
        } else if let Some(b) = line.strip_prefix("bad=") {
            BAD.with(|x| *x.borrow_mut() = b.to_owned());
        } else {
            let mut p = line.split('|');
            let (name, url, key) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or(""));
            list.push(Source { name: name.into(), url: url.into(), api_key: key.into(), enabled: true });
        }
    }
    super::store(&list);
    log_field();
    super::sources::open();
    keep("01-sources");
    let Some(m) = sources::open_window() else { return };
    set(Step::Testing);
    m.run_test();
}

/// A source's Test answered.
pub fn tested() {
    if step() != Step::Testing {
        return;
    }
    keep("02-tested");
    let Some(m) = sources::open_window() else { return };
    m.open_editor(None);
    let Some(e) = sources::editor() else { return };
    e.name.setStringValue(&ns("Broken"));
    e.url.setStringValue(&ns("ftp://nowhere"));
    e.key.setStringValue(&ns("SECRETKEY"));
    keep("03-editor");
    crate::dialogs::act(e.id, sources::EDITOR_OK);
    keep("04-editor-bad-url");
    if let Some(alert) = e.window.attachedSheet() {
        e.window.endSheet(&alert);
    }
    let bad = BAD.with(|b| b.borrow().clone());
    e.url.setStringValue(&ns(if bad.is_empty() { "http://127.0.0.1:9/api" } else { &bad }));
    crate::dialogs::act(e.id, sources::EDITOR_OK);
    keep("05-source-added");
    m.window.close();
    // the first source only
    super::set_mode(Command::SearchSource(0));
    log_field();
    keep("06-mode");
    set(Step::Searching);
    let q = QUERY.with(|q| q.borrow().clone());
    if let Some(u) = crate::app::ui() {
        u.search.setStringValue(&ns(&q));
    }
    crate::app::run(Command::Search);
}

/// Answers came in.
pub fn collected() {
    if results::running() {
        return;
    }
    match step() {
        Step::Searching => {
            keep("07-results");
            let Some(r) = results::open() else { return };
            by_name(&r.table);
            keep("08-sorted");
            select_row(&r.table, 0);
            set(Step::Adding);
            results::add_selected();
        }
        Step::SearchingAll => {
            // the torrent added before shows up in the list with the engine's next alerts
            if crate::app::peek(|c| c.list().is_empty()).unwrap_or(false) {
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_secs(1));
                    crate::mainq::on_main(collected);
                });
                return;
            }
            keep("10-all-sources");
            let Some(r) = results::open() else { return };
            by_name(&r.table);
            select_row(&r.table, 0);
            set(Step::AddingAgain);
            results::add_selected();
        }
        _ => {}
    }
}

/// Sorts by Name, A to Z (the fake source's `.torrent` result first, its broken link second).
fn by_name(t: &NSTableView) {
    let d = NSSortDescriptor::sortDescriptorWithKey_ascending(Some(&ns("search.0")), true);
    t.setSortDescriptors(&NSArray::from_retained_slice(&[d]));
}

fn select_row(t: &NSTableView, row: usize) {
    if t.numberOfRows() > row as isize {
        t.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
    }
}

/// An Add finished (added, failed, or already in the list).
pub fn added() {
    match step() {
        Step::Adding => {
            keep("09-added");
            super::set_mode(Command::SearchAllSources);
            log_field();
            set(Step::SearchingAll);
            let q = QUERY.with(|q| q.borrow().clone());
            if let Some(r) = results::open() {
                r.window.close();
            }
            super::run(&q);
        }
        Step::AddingAgain => {
            keep("11-added-again");
            let Some(r) = results::open() else { return };
            select_row(&r.table, 1);
            set(Step::AddingBroken);
            results::add_selected();
        }
        Step::AddingBroken => {
            keep("12-add-failed");
            set(Step::Done);
        }
        _ => {}
    }
}
