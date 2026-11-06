//! The Trackers, Peers and Logger tabs, and the table under the piece map and in the Files
//! tab: virtual tables (`table.rs`) over `Detail::rows` and `Detail::cell` with the tab's
//! columns (`Tab::columns`). Trackers shows each tracker's status icon before its URL; the
//! Logger is one column, scrolled to its newest line while it was at the end.

use std::cell::Cell;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSScrollView, NSTableViewColumnAutoresizingStyle};
use rookparcel_core::TorrentId;
use rookparcel_frontend::detail::{ColumnDef, Tab};
use rookparcel_frontend::icons::tracker_icon;
use rookparcel_frontend::lang;

use crate::app::peek;
use crate::images;
use crate::table::{
    self, Align, CellKind, CellValue, ColumnSpec, Hooks, RPTableSource, RPTableView, TableKind,
};
use crate::ui;

/// One detail table and what it showed last.
pub struct Table {
    pub scroll: Retained<NSScrollView>,
    pub view: Retained<RPTableView>,
    pub source: Retained<RPTableSource>,
    tab: Tab,
    /// The rows at the last refresh.
    rows: Cell<usize>,
    /// The torrent at the last refresh.
    shown: Cell<Option<TorrentId>>,
}

/// The column specs of a tab's columns (titles from the string table, widths, alignment).
pub fn specs(defs: &[ColumnDef]) -> Vec<ColumnSpec> {
    defs.iter()
        .enumerate()
        .map(|(i, &(title, width, right))| {
            let mut s = ColumnSpec::text(&format!("c{i}"), &ui::text(lang::text(title)), f64::from(width));
            s.align = if right { Align::Right } else { Align::Left };
            // the first column (name, address, message) stays
            s.hideable = i > 0;
            s
        })
        .collect()
}

/// The hooks of a plain detail table: its rows and text cells from `Detail`.
pub fn hooks(tab: Tab) -> Hooks {
    Hooks::flat(
        move || peek(|c| c.detail().rows(tab)).unwrap_or(0),
        move |row, col, _dark| {
            peek(|c| CellValue::Text(c.detail().cell(tab, row as usize, col))).unwrap_or(CellValue::Empty)
        },
    )
}

impl Table {
    pub fn new(tab: Tab, specs: Vec<ColumnSpec>, hooks: Hooks, mtm: MainThreadMarker) -> Table {
        let (scroll, view, source) = table::table(TableKind(tab.key()), specs, hooks, None, mtm);
        Table { scroll, view, source, tab, rows: Cell::new(0), shown: Cell::new(None) }
    }

    /// The Trackers tab: the status icon before each URL.
    pub fn trackers(mtm: MainThreadMarker) -> Table {
        let mut specs = specs(Tab::Trackers.columns());
        specs[0].kind = CellKind::IconText;
        let mut hooks = hooks(Tab::Trackers);
        hooks.cell = Box::new(|row, col, dark| {
            peek(|c| {
                let d = c.detail();
                let Some(t) = d.trackers.get(row as usize) else { return CellValue::Empty };
                let text = d.cell(Tab::Trackers, row as usize, col);
                if col == 0 {
                    CellValue::IconText(Some(images::state(tracker_icon(t.status), dark)), text)
                } else {
                    CellValue::Text(text)
                }
            })
            .unwrap_or(CellValue::Empty)
        });
        Table::new(Tab::Trackers, specs, hooks, mtm)
    }

    /// The Peers tab.
    pub fn peers(mtm: MainThreadMarker) -> Table {
        Table::new(Tab::Peers, specs(Tab::Peers.columns()), hooks(Tab::Peers), mtm)
    }

    /// The Logger: one column across the width, no header.
    pub fn logger(mtm: MainThreadMarker) -> Table {
        let t = Table::new(Tab::Logger, specs(Tab::Logger.columns()), hooks(Tab::Logger), mtm);
        t.view.setHeaderView(None);
        t.view.setUsesAlternatingRowBackgroundColors(false);
        t.view
            .setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle);
        t.scroll.setHasHorizontalScroller(false);
        t.view.sizeLastColumnToFit();
        t
    }

    /// The selected rows.
    pub fn selected(&self) -> Vec<usize> {
        self.source.selected().into_iter().map(|r| r as usize).collect()
    }

    /// Drops the selection (the rows mean other files now).
    pub fn deselect(&self) {
        self.source.select(&[]);
    }

    /// Shows the rows `Detail` has now: another torrent starts at the top with nothing
    /// selected, a changed count reloads every row, else the rows on screen are redrawn.
    pub fn refresh(&self) {
        let Some((n, id)) = peek(|c| (c.detail().rows(self.tab), c.detail().id)) else { return };
        let before = self.rows.replace(n);
        if self.shown.replace(id) != id {
            self.source.select(&[]);
            self.source.reload_keep_selection();
            if n > 0 {
                self.view.scrollRowToVisible(0);
            }
        } else if n != before {
            // the Logger follows its newest line while it was at the end
            let range = self.view.rowsInRect(self.view.visibleRect());
            let at_end = range.location + range.length >= before;
            self.source.reload_keep_selection();
            if self.tab == Tab::Logger && at_end && n > 0 {
                self.view.scrollRowToVisible(n as isize - 1);
            }
        } else {
            self.source.reload_visible();
        }
    }
}
