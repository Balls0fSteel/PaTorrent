//! The Advanced page's table: every engine setting by its libtorrent name (`advanced_rows`, a
//! `*` after a changed one), filtered as the filter field changes. A value is edited in place:
//! a double-click or Return on a row puts a field over its value (a switch flips at once), and
//! the field's Return or losing the focus applies it, as every Preferences control does. Reset
//! puts the selected setting back to its default. A value that is not one shows its message in
//! red under the table; the help line and the table's tooltip show the selected row's help
//! (`help::for_setting`).

use std::cell::RefCell;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSScrollView, NSSearchField, NSTextField, NSView};
use objc2_foundation::NSString;
use rookparcel_frontend::help;
use rookparcel_frontend::lang::{self, H, S};
use rookparcel_frontend::pref_pages::{advanced_name, advanced_rows};
use rookparcel_frontend::settings::{Entry, Kind, Value};
use rookparcel_frontend::tr;

use super::controls::Builder;
use super::{Act, State, state};
use crate::commands::Command;
use crate::table::{self, CellValue, ColumnSpec, Hooks, Key, RPTableSource, RPTableView, TableKind};
use crate::ui::{self, layout};

pub(crate) struct Advanced {
    pub filter: Retained<NSSearchField>,
    pub table: Retained<RPTableView>,
    pub source: Retained<RPTableSource>,
    /// Kept with the table.
    _scroll: Retained<NSScrollView>,
    _reset: Retained<NSButton>,
    /// A value's message, in red.
    error: Retained<NSTextField>,
    /// The rows the filter lets through.
    rows: RefCell<Vec<&'static Entry>>,
    /// The value being edited: its setting and the field over the cell.
    editor: RefCell<Option<(&'static Entry, Retained<NSTextField>)>>,
    editor_cmd: Command,
}

/// The table, the filter above it, the message and Reset under it.
pub(crate) fn build(b: &mut Builder) -> Retained<NSView> {
    let mtm = b.mtm;
    let filter_cmd = b.act(Act::AdvFilter);
    let filter = ui::search_field("", filter_cmd, mtm);
    layout::stretch(&filter);
    let filter_label = ui::label_tr(tr!(PrefFilter), mtm);
    b.help(&filter, H::for_label(S::PrefFilter));
    b.help(&filter_label, H::for_label(S::PrefFilter));
    let top = layout::hstack(&[&filter_label, &filter], mtm);

    let specs = vec![
        ColumnSpec::text("name", &ui::text(tr!(ColName)), 300.0),
        ColumnSpec::text("value", &ui::text(tr!(ColValue)), 160.0),
    ];
    let mut hooks = Hooks::flat(
        || state().and_then(|st| st.advanced.as_ref().map(|a| a.rows.borrow().len())).unwrap_or(0),
        |row, column, _dark| {
            state().and_then(|st| cell(&st, row as usize, column)).unwrap_or(CellValue::Empty)
        },
    );
    hooks.activate = Some(Box::new(|row| {
        if let Some(st) = state() {
            begin_edit(&st, row as usize);
        }
    }));
    hooks.key = Some(Box::new(|k, rows| match (k, rows.first(), state()) {
        (Key::Return, Some(&row), Some(st)) => {
            begin_edit(&st, row as usize);
            true
        }
        _ => false,
    }));
    let (scroll, table, source) = table::table(TableKind("advanced"), specs, hooks, None, mtm);
    table.setAllowsMultipleSelection(false);
    layout::height(&scroll, 340.0);
    layout::min_width(&scroll, 480.0);

    let error = ui::note("", mtm);
    error.setTextColor(Some(&super::error_color()));
    error.setPreferredMaxLayoutWidth(480.0);
    error.setHidden(true);
    let reset_cmd = b.act(Act::AdvReset);
    let reset = ui::button_tr(tr!(PrefReset), reset_cmd, mtm);
    b.help(&reset, H::for_label(S::PrefReset));
    let bottom = layout::hstack(&[&reset], mtm);

    let view = layout::vstack(&[&top, &scroll, &error, &bottom], mtm);
    layout::activate(&[
        top.widthAnchor().constraintEqualToAnchor(&view.widthAnchor()),
        scroll.widthAnchor().constraintEqualToAnchor(&view.widthAnchor()),
    ]);
    let editor_cmd = b.act(Act::AdvEditor);
    b.advanced = Some(Advanced {
        filter,
        table,
        source,
        _scroll: scroll,
        _reset: reset,
        error,
        rows: RefCell::new(advanced_rows("")),
        editor: RefCell::new(None),
        editor_cmd,
    });
    Retained::into_super(view)
}

/// A cell: the name (with `*` when changed) or the value as applied.
fn cell(st: &State, row: usize, column: usize) -> Option<CellValue> {
    let a = st.advanced.as_ref()?;
    let e = *a.rows.borrow().get(row)?;
    let o = st.applied.borrow();
    Some(CellValue::Text(match column {
        0 => advanced_name(e, &o.settings),
        _ => e.get(&o.settings).to_string(),
    }))
}

impl Advanced {
    /// Redraws the values (they were applied), keeping the selection.
    pub(crate) fn reload(&self) {
        self.source.reload_keep_selection();
    }

    /// The selected row's help.
    pub(crate) fn row_help(&self) -> Option<H> {
        let row = usize::try_from(self.table.selectedRow()).ok()?;
        let name = self.rows.borrow().get(row)?.name;
        help::for_setting(name)
    }

    /// Drops the value editor without applying it.
    pub(crate) fn cancel_edit(&self) {
        let editor = self.editor.borrow_mut().take();
        if let Some((_, f)) = editor {
            f.removeFromSuperview();
        }
    }

    fn show_error(&self, message: Option<&str>) {
        self.error.setStringValue(&NSString::from_str(message.unwrap_or("")));
        self.error.setHidden(message.is_none());
    }
}

/// The filter changed: the matching rows, none selected.
pub(crate) fn filter_changed(st: &State) {
    let Some(a) = &st.advanced else { return };
    a.cancel_edit();
    let rows = advanced_rows(&ui::value(&a.filter));
    *a.rows.borrow_mut() = rows;
    // SAFETY: nil sender, as a menu item's would be.
    unsafe { a.table.deselectAll(None) };
    a.table.reloadData();
}

/// Edits row `row`'s value: a switch flips, anything else gets a field over its value.
fn begin_edit(st: &State, row: usize) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(a) = &st.advanced else { return };
    let Some(e) = a.rows.borrow().get(row).copied() else { return };
    a.cancel_edit();
    let value = e.get(&st.applied.borrow().settings);
    if e.kind == Kind::Bool {
        let on = matches!(value, Value::Bool(true));
        set_value(st, e, &(!on).to_string());
        return;
    }
    let f = ui::field(&value.to_string(), "", mtm);
    // over the cell, in the table's own coordinates: it scrolls with the row
    f.setTranslatesAutoresizingMaskIntoConstraints(true);
    f.setFrame(a.table.frameOfCellAtColumn_row(1, row as isize));
    crate::ui::target::wire(&f, a.editor_cmd, mtm);
    if let Some(cell) = f.cell() {
        cell.setSendsActionOnEndEditing(true);
    }
    ui::tooltip(&f, help::for_setting(e.name).map_or("", lang::help));
    a.table.addSubview(&f);
    *a.editor.borrow_mut() = Some((e, f.clone()));
    st.window.makeFirstResponder(Some(&f));
}

/// The value editor committed (Return or the focus left it).
pub(crate) fn end_edit(st: &State) {
    let Some(a) = &st.advanced else { return };
    let editor = a.editor.borrow_mut().take();
    let Some((e, f)) = editor else { return };
    let text = ui::value(&f);
    // ending its editing sends its action once more, which finds no editor
    f.removeFromSuperview();
    let lost = st.window.firstResponder().is_none_or(|r| {
        let w: &objc2::runtime::AnyObject = st.window.as_ref();
        std::ptr::eq::<objc2::runtime::AnyObject>(r.as_ref(), w)
    });
    if lost {
        st.window.makeFirstResponder(Some(&a.table));
    }
    set_value(st, e, &text);
}

/// Reset: the selected setting's default.
pub(crate) fn reset(st: &State) {
    let Some(a) = &st.advanced else { return };
    let Ok(row) = usize::try_from(a.table.selectedRow()) else { return };
    let Some(e) = a.rows.borrow().get(row).copied() else { return };
    a.cancel_edit();
    let default = e.get(&rookparcel_core::Settings::default()).to_string();
    set_value(st, e, &default);
}

/// Applies `text` to setting `e`; a value that is not one shows its message.
fn set_value(st: &State, e: &'static Entry, text: &str) {
    let Some(a) = &st.advanced else { return };
    let mut o = st.applied.borrow().clone();
    match e.set(&mut o.settings, text) {
        Err(message) => a.show_error(Some(&message)),
        Ok(()) => {
            a.show_error(None);
            if o != *st.applied.borrow() {
                super::apply(st, o);
            }
        }
    }
}
