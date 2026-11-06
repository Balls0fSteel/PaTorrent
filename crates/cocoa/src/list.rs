//! The torrent list over `Core`'s `TorrentList` (GTK `list.rs`): the 24 columns of
//! `frontend::columns` in the prefs' order, widths and visibility (`prefs.columns()`, written
//! back as they change), the state icon before the name, the Done bar, sorting kept in prefs,
//! the selection driving the detail tabs, double-click, Return, ⌫, the context menu (the Torrent
//! menu) and `.torrent` files dropped on the window.

use std::collections::HashSet;
use std::path::PathBuf;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSScrollView;
use rookparcel_core::TorrentId;
use rookparcel_frontend::columns::{Align as ColAlign, Column, RowExtra};
use rookparcel_frontend::status::status_icon;

use crate::app::{apply, log_line, peek, with_core};
use crate::commands::Command;
use crate::table::{
    self, Align, CellKind, CellValue, ColumnSpec, Hooks, Key, RPTableSource, RPTableView, TableKind,
};
use crate::{canvas, images, menu};

/// The torrent table: the columns in the prefs' order and widths, over `list.view()`, sorted
/// by `sort` (column, descending).
pub fn build(
    columns: &[(Column, i32)],
    sort: Option<(Column, bool)>,
    alternate_rows: bool,
    mtm: MainThreadMarker,
) -> (Retained<NSScrollView>, Retained<RPTableView>, Retained<RPTableSource>) {
    let cols: Vec<Column> = columns.iter().map(|(c, _)| *c).collect();
    let specs: Vec<ColumnSpec> = columns
        .iter()
        .map(|&(c, w)| ColumnSpec {
            id: c.key().to_owned(),
            title: c.title().to_owned(),
            width: f64::from(w.unsigned_abs()),
            min_width: 24.0,
            align: if c.align() == ColAlign::Right { Align::Right } else { Align::Left },
            kind: match c {
                Column::Name => CellKind::IconText,
                Column::Done => CellKind::Bar,
                _ => CellKind::Text,
            },
            visible: w > 0,
            hideable: c != Column::Name,
            sortable: true,
            descending_first: c.default_descending(),
        })
        .collect();
    let cells = cols.clone();
    let mut hooks = Hooks::flat(
        || peek(|c| c.list().view().len()).unwrap_or(0),
        move |row, i, dark| {
            let col = cells[i];
            peek(|c| {
                let Some(&id) = c.list().view().get(row as usize) else { return CellValue::Empty };
                let Some(st) = c.list().get(id) else { return CellValue::Empty };
                let x: RowExtra<'_> = c.list().extra(id);
                match col {
                    Column::Name => CellValue::IconText(
                        Some(images::state(status_icon(st, x.user_paused), dark)),
                        st.name.clone(),
                    ),
                    Column::Done => CellValue::Bar(canvas::Bar {
                        fraction: f64::from(st.progress_ppm) / 1e6,
                        complete: st.is_finished,
                        text: col.text(st, x),
                    }),
                    _ => CellValue::Text(col.text(st, x)),
                }
            })
            .unwrap_or(CellValue::Empty)
        },
    );
    hooks.id = Box::new(|row| peek(|c| c.list().view().get(row).map(|id| id.to_raw())).flatten());
    // type-select by Name, as Finder lists do
    hooks.type_select = Some(Box::new(|row| {
        peek(|c| {
            let id = c.list().view().get(row as usize)?;
            c.list().get(*id).map(|st| st.name.clone())
        })
        .flatten()
    }));
    hooks.rows_of = Box::new(|ids| {
        let want: HashSet<u64> = ids.iter().copied().collect();
        peek(|c| {
            c.list()
                .view()
                .iter()
                .enumerate()
                .filter(|(_, id)| want.contains(&id.to_raw()))
                .map(|(r, _)| r)
                .collect()
        })
        .unwrap_or_default()
    });
    let sorted = cols.clone();
    hooks.sort = Some(Box::new(move |(i, ascending)| {
        let col = sorted[i];
        with_core(|c| {
            let list = c.list_mut();
            list.set_sort(Some((col, !ascending)));
            // the table reloads right after this hook: the new order must be there
            list.refresh();
            let sort = list.sort();
            c.prefs_mut().set_sort(sort);
            c.save_prefs();
        });
    }));
    let laid = cols.clone();
    hooks.layout = Some(Box::new(move |order| {
        let mut out: Vec<(Column, i32)> =
            order.iter().map(|&(i, w, v)| (laid[i], if v { w as i32 } else { -(w as i32) })).collect();
        for c in &laid {
            if !out.iter().any(|(d, _)| d == c) {
                out.push((*c, -c.default_width()));
            }
        }
        with_core(|c| c.prefs_mut().set_columns(&out));
    }));
    hooks.selection = Some(Box::new(selection));
    hooks.activate = Some(Box::new(activate));
    hooks.key = Some(Box::new(key));
    hooks.menu = Some(Box::new(|_ids| {
        let mtm = MainThreadMarker::new()?;
        Some(menu::build("", &menu::torrent(&crate::app::labels(), false), mtm))
    }));
    let sort = sort.and_then(|(col, desc)| cols.iter().position(|&c| c == col).map(|i| (i, !desc)));
    let (scroll, view, source) = table::table(TableKind("torrents"), specs, hooks, sort, mtm);
    view.setUsesAlternatingRowBackgroundColors(alternate_rows);
    (scroll, view, source)
}

fn ids_of(raw: &[u64]) -> Vec<TorrentId> {
    raw.iter().map(|&i| TorrentId::from_raw(i)).collect()
}

/// The selection changed: the detail tabs show its first torrent.
fn selection(ids: &[u64]) {
    let ids = ids_of(ids);
    with_core(|c| c.select(&ids));
    crate::detail::refresh();
}

/// A double click: a finished torrent's folder, otherwise Properties.
pub fn activate(id: u64) {
    let id = TorrentId::from_raw(id);
    if peek(|c| c.is_finished(id)) == Some(true) {
        crate::torrent_menu::open_folder(&[id]);
    } else {
        crate::dialogs::properties::open(&[id.to_raw()]);
    }
}

/// Return opens Properties, ⌫ removes (with Remove's question).
pub fn key(key: Key, ids: &[u64]) -> bool {
    if ids.is_empty() {
        return false;
    }
    match key {
        Key::Return => crate::app::run(Command::Properties),
        Key::Delete => crate::app::run(Command::Remove),
    }
    true
}

/// `.torrent` files dropped on the window: each is added as File > Add Torrent adds it (the
/// Add New Torrent dialog when Preferences ask for it).
pub fn add_files(paths: Vec<PathBuf>) {
    for p in paths {
        match std::fs::read(&p) {
            Ok(bytes) => {
                if let Some(step) = with_core(|c| c.add_file(bytes)) {
                    crate::dialogs::add::handle(step);
                }
            }
            Err(e) => log_line(&format!("{}: {e}", p.display())),
        }
    }
    apply(with_core(|c| c.events()));
}
