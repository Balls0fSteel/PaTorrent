//! The sidebar, µTorrent 2.2.1's category tree as a source list (GTK `sidebar.rs`): Torrents
//! with Downloading, Completed, Active and Inactive under it, the Labels heading with No Label
//! and every label in use, then Feeds (every feed's items) with each feed (slice H's
//! `rss::sidebar_rows`). Each row shows its count; a pick shows those torrents
//! (`Core::show_torrents`) or a feed's items (`rss::show_feed`).
//!
//! Rows are `RPTableSource` outline items keyed by a `u64`: fixed ids for the fixed rows, a label
//! keeps the id it first got while the process runs (so the selection survives a label coming or
//! going), a feed's id is its `FeedId` above [`FEED_BASE`].

use std::cell::RefCell;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSOutlineView, NSScrollView};
use objc2_foundation::NSNumber;
use rookparcel_frontend::icons::{UiIcon, category_icon};
use rookparcel_frontend::model::Filter;
use rookparcel_frontend::rss::FeedId;
use rookparcel_frontend::status::Category;
use rookparcel_frontend::tr;

use crate::app::{apply, peek, ui, with_core};
use crate::table::{self, CellKind, CellValue, ColumnSpec, Hooks, RPTableSource, TableKind};
use crate::{images, mainq};

/// Torrents (every torrent); the other categories follow it: `TORRENTS + i` is
/// `Category::ALL[i]`.
const TORRENTS: u64 = 1;
/// The Labels heading.
const LABELS: u64 = 10;
const NO_LABEL: u64 = 11;
/// The Feeds row: every feed's items.
const FEEDS: u64 = 20;
/// A label's row: `LABEL_BASE` + its index in `State::known`.
const LABEL_BASE: u64 = 1 << 20;
/// A feed's row: `FEED_BASE` + its id.
const FEED_BASE: u64 = 1 << 40;

/// What a row shows in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pick {
    Torrents(Filter),
    /// A feed's items (`None`: every feed's).
    Feed(Option<FeedId>),
}

#[derive(Default)]
struct State {
    /// Every label name given a row id so far (the id is `LABEL_BASE` + the index).
    known: Vec<String>,
    /// The labels in use, by name, with their counts.
    labels: Vec<(String, usize)>,
    /// How many torrents have no label.
    unlabeled: usize,
    feeds: Vec<(FeedId, String)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

impl State {
    fn label_id(&mut self, name: &str) -> u64 {
        let i = match self.known.iter().position(|k| k == name) {
            Some(i) => i,
            None => {
                self.known.push(name.to_owned());
                self.known.len() - 1
            }
        };
        LABEL_BASE + i as u64
    }

    fn label_name(&self, id: u64) -> Option<&str> {
        let i = usize::try_from(id.checked_sub(LABEL_BASE)?).ok()?;
        self.known.get(i).map(String::as_str)
    }
}

/// What row `id` picks; `None` for the heading.
fn pick_of(id: u64) -> Option<Pick> {
    match id {
        TORRENTS..=5 => Some(Pick::Torrents(Filter::Category(Category::ALL[(id - TORRENTS) as usize]))),
        NO_LABEL => Some(Pick::Torrents(Filter::Label(String::new()))),
        FEEDS => Some(Pick::Feed(None)),
        _ if id >= FEED_BASE => FeedId::try_from(id - FEED_BASE).ok().map(|f| Pick::Feed(Some(f))),
        _ if id >= LABEL_BASE => STATE
            .with(|s| s.borrow().label_name(id).map(str::to_owned))
            .map(|l| Pick::Torrents(Filter::Label(l))),
        _ => None,
    }
}

/// A row's children (`None`: the top level).
fn children(parent: Option<u64>) -> Vec<u64> {
    match parent {
        None => vec![TORRENTS, LABELS, FEEDS],
        Some(TORRENTS) => (TORRENTS + 1..TORRENTS + Category::ALL.len() as u64).collect(),
        Some(LABELS) => STATE.with(|s| {
            let mut s = s.borrow_mut();
            let names: Vec<String> = s.labels.iter().map(|(l, _)| l.clone()).collect();
            let mut ids = vec![NO_LABEL];
            ids.extend(names.iter().map(|l| s.label_id(l)));
            ids
        }),
        Some(FEEDS) => {
            STATE.with(|s| s.borrow().feeds.iter().map(|(f, _)| FEED_BASE + u64::from(*f)).collect())
        }
        Some(_) => Vec::new(),
    }
}

/// "Name (count)".
fn counted(name: &str, n: usize) -> String {
    format!("{name} ({n})")
}

fn cell(id: u64, dark: bool) -> CellValue {
    let icon = |i: UiIcon| Some(images::ui(i, dark));
    match id {
        TORRENTS..=5 => {
            let c = Category::ALL[(id - TORRENTS) as usize];
            let n = peek(|core| core.list().count(c)).unwrap_or(0);
            CellValue::IconText(icon(category_icon(c)), counted(&crate::ui::text(c.label()), n))
        }
        LABELS => CellValue::Text(crate::ui::text(tr!(CatLabels))),
        NO_LABEL => {
            let n = STATE.with(|s| s.borrow().unlabeled);
            CellValue::IconText(icon(UiIcon::NoLabel), counted(&crate::ui::text(tr!(CatNoLabel)), n))
        }
        FEEDS => CellValue::IconText(icon(UiIcon::Feeds), crate::ui::text(tr!(CatFeeds))),
        _ if id >= FEED_BASE => {
            let name = STATE.with(|s| {
                s.borrow().feeds.iter().find(|(f, _)| FEED_BASE + u64::from(*f) == id).map(|(_, n)| n.clone())
            });
            name.map_or(CellValue::Empty, |n| CellValue::IconText(icon(UiIcon::Feeds), n))
        }
        _ if id >= LABEL_BASE => {
            // a label is the user's text, shown as it is
            let text = STATE.with(|s| {
                let s = s.borrow();
                let name = s.label_name(id)?;
                let n = s.labels.iter().find(|(l, _)| l == name).map_or(0, |(_, n)| *n);
                Some(counted(name, n))
            });
            text.map_or(CellValue::Empty, |t| CellValue::IconText(icon(UiIcon::Labels), t))
        }
        _ => CellValue::Empty,
    }
}

/// The rows showing these ids (the outline's rows as they are now, expanded or not).
fn rows_of(ids: &[u64]) -> Vec<usize> {
    let Some(u) = ui() else { return Vec::new() };
    let ov = &u.sidebar_view;
    (0..ov.numberOfRows())
        .filter(|&r| {
            ov.itemAtRow(r)
                .and_then(|i| i.downcast_ref::<NSNumber>().map(|n| n.as_u64()))
                .is_some_and(|id| ids.contains(&id))
        })
        .map(|r| r as usize)
        .collect()
}

/// The sidebar outline (a source list) with its rows from `Core`.
pub fn build(
    mtm: MainThreadMarker,
) -> (Retained<NSScrollView>, Retained<NSOutlineView>, Retained<RPTableSource>) {
    refresh_state();
    let mut spec = ColumnSpec::text("name", "", 160.0);
    spec.kind = CellKind::IconText;
    spec.hideable = false;
    let mut hooks = Hooks::flat(|| 0, |id, _, dark| cell(id, dark));
    hooks.children = Some(Box::new(children));
    hooks.is_group = Some(Box::new(|id| id == LABELS));
    hooks.rows_of = Box::new(rows_of);
    hooks.selection = Some(Box::new(select));
    let (scroll, outline, source) = table::outline(TableKind("sidebar"), vec![spec], hooks, true, mtm);
    outline.setAllowsEmptySelection(false);
    (scroll, outline, source)
}

/// Selects Torrents (at the start).
pub fn select_all_torrents() {
    if let Some(u) = ui() {
        u.sidebar.select(&[TORRENTS]);
    }
}

/// Reads the labels, their counts and the feeds; true when the rows change.
fn refresh_state() -> bool {
    let (labels, unlabeled) = peek(|c| (c.list().labels(), c.list().unlabeled())).unwrap_or_default();
    let feeds = crate::rss::sidebar_rows();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let rows_changed = s.feeds != feeds
            || s.labels.len() != labels.len()
            || s.labels.iter().zip(&labels).any(|((a, _), (b, _))| a != b);
        s.labels = labels;
        s.unlabeled = unlabeled;
        s.feeds = feeds;
        rows_changed
    })
}

/// `Events::sidebar`: new counts; the rows again when the labels or feeds changed.
pub fn update() {
    let changed = refresh_state();
    let Some(u) = ui() else { return };
    if changed {
        u.sidebar.reload_keep_selection();
        // SAFETY: nil expands every top-level item and its children (new labels and feeds show).
        unsafe { u.sidebar_view.expandItem_expandChildren(None, true) };
    } else {
        u.sidebar.reload_visible();
    }
}

/// The user (or a reload that lost the row) picked a row.
pub fn select(ids: &[u64]) {
    let Some(&id) = ids.first() else {
        // what was shown went away (a label no torrent has now): back to Torrents, once this
        // selection change is over
        mainq::on_main(select_all_torrents);
        return;
    };
    match pick_of(id) {
        Some(Pick::Torrents(f)) => apply(with_core(|c| {
            c.show_torrents(f);
            c.events()
        })),
        Some(Pick::Feed(f)) => crate::rss::show_feed(f),
        None => {}
    }
}
