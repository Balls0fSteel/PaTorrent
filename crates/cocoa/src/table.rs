//! `RPTableSource`: the one data source and delegate of every list (convention 1), over a
//! view-based `NSTableView` (a list of rows) or `NSOutlineView` (a tree of ids). It is virtual:
//! AppKit asks for the rows on screen only, and cell views are reused
//! (`makeViewWithIdentifier:owner:`), so 100,000 torrents cost nothing until they scroll by.
//!
//! A table is made of column specs ([`ColumnSpec`]: title, width, alignment, cell kind,
//! visibility, sortable) and [`Hooks`], closures that read the application's state (a shared
//! borrow only: AppKit calls the data source synchronously, also from inside `reloadData`) and
//! act on clicks. What the table gives:
//!
//! - sort descriptors: a click on a header calls `hooks.sort(column, ascending)`, then reloads
//!   keeping the selection;
//! - the header's menu shows and hides the columns that may hide; moving, resizing, showing or
//!   hiding one calls `hooks.layout` (to keep it in prefs);
//! - selection: `hooks.selection(ids)` with the selected rows' ids; double-click
//!   (`hooks.activate`), Return and ⌫ (`hooks.key`, through `RPTableView`'s `keyDown:`), the
//!   context menu (`hooks.menu`, through `RPTableView`'s `menuForEvent:`);
//! - [`RPTableSource::reload_visible`] redraws the rows on screen (the 1 s refresh) and
//!   [`RPTableSource::reload_keep_selection`] reloads everything and selects the same ids again
//!   (after the rows moved).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSButton, NSControl, NSControlStateValueOff, NSControlStateValueOn, NSControlTextEditingDelegate,
    NSEvent, NSImage, NSImageView, NSLineBreakMode, NSMenu, NSMenuItem, NSMenuItemValidation, NSOutlineView,
    NSOutlineViewDataSource, NSOutlineViewDelegate, NSResponder, NSTableCellView, NSTableColumn, NSTableView,
    NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle, NSTextAlignment, NSTextField,
    NSUserInterfaceItemIdentification, NSView,
};
use objc2_foundation::{
    NSArray, NSIndexSet, NSMutableIndexSet, NSNotFound, NSNotification, NSNumber, NSPoint, NSRange, NSRect,
    NSSize, NSSortDescriptor, NSString,
};

use crate::canvas::{self, RPCanvas};
use crate::ui::controls::auto;

/// Which list a source serves (logs, snapshots); any name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TableKind(pub &'static str);

/// How a column's cells look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    Text,
    /// An image before the text (the Name column's state icon).
    IconText,
    /// A drawn bar (`canvas::Bar`).
    Bar,
    /// A canvas of this kind (`CellValue::Drawn`), as the Pieces list's Blocks cells.
    Drawn(canvas::Kind),
    /// A check box with the text as its title (`CellValue::Check`); a click calls `hooks.check`
    /// (the RSS Downloader's filter list).
    Check,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

/// One column.
#[derive(Clone, Debug)]
pub struct ColumnSpec {
    /// The column's identifier, unique in its table (also the sort descriptor's key).
    pub id: String,
    pub title: String,
    pub width: f64,
    pub min_width: f64,
    pub align: Align,
    pub kind: CellKind,
    pub visible: bool,
    /// Whether the header's menu may hide it (the Name column stays).
    pub hideable: bool,
    pub sortable: bool,
    /// The first click on its header sorts descending ("more is interesting" columns).
    pub descending_first: bool,
}

impl ColumnSpec {
    pub fn text(id: &str, title: &str, width: f64) -> ColumnSpec {
        ColumnSpec {
            id: id.to_owned(),
            title: title.to_owned(),
            width,
            min_width: 24.0,
            align: Align::Left,
            kind: CellKind::Text,
            visible: true,
            hideable: true,
            sortable: false,
            descending_first: false,
        }
    }
}

/// A cell's content.
pub enum CellValue {
    Text(String),
    IconText(Option<Retained<NSImage>>, String),
    Bar(canvas::Bar),
    /// A `CellKind::Drawn` cell's value and the text VoiceOver reads for it.
    Drawn(Box<dyn std::any::Any>, String),
    /// A `CellKind::Check` cell: on or off, and its title.
    Check(bool, String),
    Empty,
}

/// Keys the list handles itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Return or Enter.
    Return,
    /// ⌫ or ⌦.
    Delete,
}

/// A column as shown: (spec index, width, visible).
pub type ColumnState = (usize, f64, bool);

/// A hook taking one value.
pub type Hook<A, R> = Box<dyn Fn(A) -> R>;
/// A hook taking ids (read during the call only).
pub type IdsHook<R> = Box<dyn Fn(&[u64]) -> R>;
/// The Return / ⌫ hook: the key and the selected ids; true when handled.
pub type KeyHook = Box<dyn Fn(Key, &[u64]) -> bool>;
/// The context menu hook: the selected ids.
pub type MenuHook = IdsHook<Option<Retained<NSMenu>>>;

/// What the table asks the application. Rows are indices into the shown order; ids are the
/// application's stable keys (a torrent id), used to keep the selection over reloads.
pub struct Hooks {
    /// The number of rows (a flat table).
    pub rows: Box<dyn Fn() -> usize>,
    /// A cell: (row, column spec index, dark appearance). For a tree, row is the item's id.
    pub cell: Box<dyn Fn(u64, usize, bool) -> CellValue>,
    /// A row's id (a flat table).
    pub id: Box<dyn Fn(usize) -> Option<u64>>,
    /// The rows of these ids, in any order (one pass over the rows: a ⌘A of 100,000 rows comes
    /// back here).
    pub rows_of: IdsHook<Vec<usize>>,
    /// A tree's children of an item (`None`: the top); `None` for a flat table.
    pub children: Option<Hook<Option<u64>, Vec<u64>>>,
    /// A tree's group rows (source-list headings).
    pub is_group: Option<Box<dyn Fn(u64) -> bool>>,
    pub sort: Option<Hook<(usize, bool), ()>>,
    pub selection: Option<IdsHook<()>>,
    pub activate: Option<Hook<u64, ()>>,
    /// Return or ⌫ with the selection's ids; true when handled.
    pub key: Option<KeyHook>,
    pub menu: Option<MenuHook>,
    /// The columns in display order: (spec index, width, visible).
    pub layout: Option<Hook<Vec<ColumnState>, ()>>,
    /// A `CellKind::Check` cell was clicked: (row's id, its new state).
    pub check: Option<Hook<(u64, bool), ()>>,
    /// The text type-select matches (typing a name's first letters selects the next row
    /// starting with them): a row's name, by row as `cell` takes it (an item's id in a tree).
    /// None: the list has no type-select.
    pub type_select: Option<Hook<u64, Option<String>>>,
}

impl Hooks {
    /// Hooks of a flat table with `rows` rows whose ids are their indices.
    pub fn flat(
        rows: impl Fn() -> usize + 'static,
        cell: impl Fn(u64, usize, bool) -> CellValue + 'static,
    ) -> Hooks {
        Hooks {
            rows: Box::new(rows),
            cell: Box::new(cell),
            id: Box::new(|r| Some(r as u64)),
            rows_of: Box::new(|ids| ids.iter().map(|&i| i as usize).collect()),
            children: None,
            is_group: None,
            sort: None,
            selection: None,
            activate: None,
            key: None,
            menu: None,
            layout: None,
            check: None,
            type_select: None,
        }
    }
}

pub struct Ivars {
    kind: TableKind,
    specs: Vec<ColumnSpec>,
    hooks: Hooks,
    /// The columns in spec order.
    columns: RefCell<Vec<Retained<NSTableColumn>>>,
    view: RefCell<Weak<NSTableView>>,
    /// The selected ids, as of the last selection change (a reload maps them back to rows).
    selected: RefCell<Vec<u64>>,
    /// Set while a reload puts the selection back: its selection changes are not the user's.
    restoring: Cell<bool>,
    /// A tree's items: one `NSNumber` per id, so AppKit sees the same object each time.
    items: RefCell<HashMap<u64, Retained<NSNumber>>>,
    /// A tree's children as last counted, by parent (`None`: the top). `numberOfChildrenOfItem:`
    /// refreshes an entry, and AppKit then asks for the children one index at a time
    /// (`child:ofItem:`), which would otherwise build the whole list again for each child: a
    /// folder of 50,000 files in the Add dialog took 50,000 lists of 50,000.
    counted: RefCell<HashMap<Option<u64>, Vec<u64>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and RPTableSource does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RPTableSource"]
    #[ivars = Ivars]
    pub struct RPTableSource;

    impl RPTableSource {
        /// The header menu's items.
        #[unsafe(method(toggleColumn:))]
        fn toggle_column(&self, sender: &NSMenuItem) {
            let i = sender.tag() as usize;
            if let Some(c) = self.ivars().columns.borrow().get(i) {
                c.setHidden(!c.isHidden());
            }
            self.layout_changed();
        }

        #[unsafe(method(doubleClick:))]
        fn double_click(&self, _sender: Option<&AnyObject>) {
            let Some(tv) = self.table() else { return };
            let row = tv.clickedRow();
            if row < 0 {
                return;
            }
            let Some(id) = self.id_of_row(&tv, row as usize) else { return };
            if let Some(f) = &self.ivars().hooks.activate {
                f(id);
            }
        }

        /// A `CellKind::Check` cell's check box was clicked.
        #[unsafe(method(checkClicked:))]
        fn check_clicked(&self, sender: &NSButton) {
            let Some(tv) = self.table() else { return };
            let row = tv.rowForView(sender);
            if row < 0 {
                return;
            }
            let Some(id) = self.id_of_row(&tv, row as usize) else { return };
            if let Some(f) = &self.ivars().hooks.check {
                f((id, sender.state() == NSControlStateValueOn));
            }
        }
    }

    unsafe impl NSObjectProtocol for RPTableSource {}

    unsafe impl NSMenuItemValidation for RPTableSource {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            if item.action() == Some(sel!(toggleColumn:)) {
                let shown = self.ivars().columns.borrow().get(item.tag() as usize).is_some_and(|c| !c.isHidden());
                item.setState(if shown { NSControlStateValueOn } else { NSControlStateValueOff });
            }
            true
        }
    }

    unsafe impl NSTableViewDataSource for RPTableSource {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _tv: &NSTableView) -> isize {
            (self.ivars().hooks.rows)() as isize
        }

        #[unsafe(method(tableView:sortDescriptorsDidChange:))]
        fn sort_changed(&self, tv: &NSTableView, _old: &NSArray<NSSortDescriptor>) {
            let Some(d) = tv.sortDescriptors().firstObject() else { return };
            let Some(key) = d.key() else { return };
            let key = key.to_string();
            let Some(i) = self.ivars().specs.iter().position(|s| s.id == key) else { return };
            if let Some(f) = &self.ivars().hooks.sort {
                f((i, d.ascending()));
            }
            self.reload_keep_selection();
        }
    }

    unsafe impl NSControlTextEditingDelegate for RPTableSource {}

    unsafe impl NSTableViewDelegate for RPTableSource {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(&self, tv: &NSTableView, column: Option<&NSTableColumn>, row: isize) -> Option<Retained<NSView>> {
            self.row_view(tv, column, row)
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _n: &NSNotification) {
            self.selection_did_change();
        }

        #[unsafe(method(tableViewColumnDidMove:))]
        fn column_moved(&self, _n: &NSNotification) {
            self.layout_changed();
        }

        #[unsafe(method(tableViewColumnDidResize:))]
        fn column_resized(&self, _n: &NSNotification) {
            self.layout_changed();
        }

        /// Type-select in a view-based table: AppKit cannot read the cells' text itself.
        #[unsafe(method_id(tableView:typeSelectStringForTableColumn:row:))]
        fn type_select_row(
            &self,
            _tv: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSString>> {
            self.type_select(u64::try_from(row).ok())
        }
    }

    unsafe impl NSOutlineViewDataSource for RPTableSource {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn children_count(&self, _ov: &NSOutlineView, item: Option<&AnyObject>) -> isize {
            let children = self.children(item);
            let n = children.len();
            if let Some(key) = parent_key(item) {
                self.ivars().counted.borrow_mut().insert(key, children);
            }
            n as isize
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn child(&self, _ov: &NSOutlineView, index: isize, item: Option<&AnyObject>) -> Retained<AnyObject> {
            let index = index as usize;
            let counted = parent_key(item)
                .and_then(|key| self.ivars().counted.borrow().get(&key).map(|c| c.get(index).copied()));
            let id = match counted {
                Some(id) => id,
                None => self.children(item).get(index).copied(),
            };
            self.item(id.unwrap_or(0))
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn expandable(&self, _ov: &NSOutlineView, item: &AnyObject) -> bool {
            !self.children(Some(item)).is_empty()
        }
    }

    unsafe impl NSOutlineViewDelegate for RPTableSource {
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        fn outline_view_for(
            &self,
            ov: &NSOutlineView,
            column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            self.item_view(ov, column, item)
        }

        #[unsafe(method(outlineView:isGroupItem:))]
        fn is_group_item(&self, _ov: &NSOutlineView, item: &AnyObject) -> bool {
            self.group(item)
        }

        #[unsafe(method(outlineView:shouldSelectItem:))]
        fn should_select(&self, _ov: &NSOutlineView, item: &AnyObject) -> bool {
            !self.group(item)
        }

        #[unsafe(method(outlineViewSelectionDidChange:))]
        fn outline_selection_changed(&self, _n: &NSNotification) {
            self.selection_did_change();
        }

        /// Type-select in the outline, by item.
        #[unsafe(method_id(outlineView:typeSelectStringForTableColumn:item:))]
        fn type_select_item(
            &self,
            _ov: &NSOutlineView,
            _column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSString>> {
            self.type_select(item_id(item))
        }
    }
);

/// A tree item's id.
fn item_id(item: &AnyObject) -> Option<u64> {
    item.downcast_ref::<NSNumber>().map(|n| n.as_u64())
}

/// The key of `counted` for a parent item (`Some(None)`: the top; `None`: not one of ours).
fn parent_key(item: Option<&AnyObject>) -> Option<Option<u64>> {
    match item {
        None => Some(None),
        Some(i) => item_id(i).map(Some),
    }
}

/// Identifiers of reused cell views: the column's id plus the cell kind.
fn reuse_id(spec: &ColumnSpec) -> Retained<NSString> {
    NSString::from_str(&format!("{}:{:?}", spec.id, spec.kind))
}

impl RPTableSource {
    fn new(
        kind: TableKind,
        specs: Vec<ColumnSpec>,
        hooks: Hooks,
        mtm: MainThreadMarker,
    ) -> Retained<RPTableSource> {
        let this = RPTableSource::alloc(mtm).set_ivars(Ivars {
            kind,
            specs,
            hooks,
            columns: RefCell::new(Vec::new()),
            view: RefCell::new(Weak::default()),
            selected: RefCell::new(Vec::new()),
            restoring: Cell::new(false),
            items: RefCell::new(HashMap::new()),
            counted: RefCell::new(HashMap::new()),
        });
        // SAFETY: NSObject's `init` on a fresh object with its ivars set.
        unsafe { msg_send![super(this), init] }
    }

    pub fn kind(&self) -> TableKind {
        self.ivars().kind
    }

    /// The table or outline view this source serves.
    pub fn table(&self) -> Option<Retained<NSTableView>> {
        self.ivars().view.borrow().load()
    }

    fn row_view(
        &self,
        tv: &NSTableView,
        column: Option<&NSTableColumn>,
        row: isize,
    ) -> Option<Retained<NSView>> {
        // a flat table's cell hook takes the row, a tree's the item's id
        self.cell_view(tv, column?, u64::try_from(row).ok()?)
    }

    fn item_view(
        &self,
        ov: &NSOutlineView,
        column: Option<&NSTableColumn>,
        item: &AnyObject,
    ) -> Option<Retained<NSView>> {
        let id = item_id(item)?;
        let column = match column {
            Some(c) => c.retain(),
            None => ov.tableColumns().firstObject()?,
        };
        self.cell_view(ov, &column, id)
    }

    /// The text type-select matches for a row (a flat table) or an item's id (a tree), whatever
    /// column AppKit asks about: the hook's, as the list's names.
    fn type_select(&self, id: Option<u64>) -> Option<Retained<NSString>> {
        let f = self.ivars().hooks.type_select.as_ref()?;
        f(id?).map(|s| NSString::from_str(&s))
    }

    fn group(&self, item: &AnyObject) -> bool {
        match (&self.ivars().hooks.is_group, item_id(item)) {
            (Some(f), Some(id)) => f(id),
            _ => false,
        }
    }

    fn children(&self, item: Option<&AnyObject>) -> Vec<u64> {
        let Some(f) = &self.ivars().hooks.children else { return Vec::new() };
        match item {
            None => f(None),
            Some(i) => item_id(i).map_or_else(Vec::new, |id| f(Some(id))),
        }
    }

    fn item(&self, id: u64) -> Retained<AnyObject> {
        let n = self.ivars().items.borrow_mut().entry(id).or_insert_with(|| NSNumber::new_u64(id)).clone();
        Retained::into_super(Retained::into_super(Retained::into_super(n)))
    }

    /// The id of a row: the hook's for a flat table, the item's for a tree.
    fn id_of_row(&self, tv: &NSTableView, row: usize) -> Option<u64> {
        if self.ivars().hooks.children.is_some() {
            let ov = tv.downcast_ref::<NSOutlineView>()?;
            return ov.itemAtRow(row as isize).as_deref().and_then(item_id);
        }
        (self.ivars().hooks.id)(row)
    }

    fn spec_of(&self, column: &NSTableColumn) -> Option<usize> {
        self.ivars().columns.borrow().iter().position(|c| std::ptr::eq(&**c, column))
    }

    fn cell_view(&self, tv: &NSTableView, column: &NSTableColumn, id: u64) -> Option<Retained<NSView>> {
        let i = self.spec_of(column)?;
        let spec = &self.ivars().specs[i];
        let mtm = MainThreadMarker::from(self);
        let reuse = reuse_id(spec);
        // SAFETY: the identifier is an NSString; no owner (no NIB).
        let old = unsafe { tv.makeViewWithIdentifier_owner(&reuse, None) };
        let view = match old {
            Some(v) => v,
            None => {
                let v = make_cell(spec, mtm);
                v.setIdentifier(Some(&reuse));
                if let Some(b) = check_box(&v) {
                    // SAFETY: `checkClicked:` is this source's action; the button keeps its target
                    // weakly, and the source outlives its table's cells.
                    unsafe {
                        b.setTarget(Some(self));
                        b.setAction(Some(sel!(checkClicked:)));
                    }
                }
                v
            }
        };
        let value = (self.ivars().hooks.cell)(id, i, crate::images::is_dark(tv));
        fill_cell(&view, value);
        Some(view)
    }

    fn selection_did_change(&self) {
        if self.ivars().restoring.get() {
            return;
        }
        let Some(tv) = self.table() else { return };
        let rows = rows_of_set(&tv.selectedRowIndexes());
        let ids: Vec<u64> = rows.into_iter().filter_map(|r| self.id_of_row(&tv, r)).collect();
        *self.ivars().selected.borrow_mut() = ids.clone();
        if let Some(f) = &self.ivars().hooks.selection {
            f(&ids);
        }
    }

    /// The selected ids.
    pub fn selected(&self) -> Vec<u64> {
        self.ivars().selected.borrow().clone()
    }

    fn layout_changed(&self) {
        let Some(f) = &self.ivars().hooks.layout else { return };
        let Some(tv) = self.table() else { return };
        let columns = tv.tableColumns();
        let order: Vec<(usize, f64, bool)> =
            columns.iter().filter_map(|c| self.spec_of(&c).map(|i| (i, c.width(), !c.isHidden()))).collect();
        f(order);
    }

    /// Redraws the rows on screen (their values changed, not their order).
    pub fn reload_visible(&self) {
        let Some(tv) = self.table() else { return };
        let range = tv.rowsInRect(tv.visibleRect());
        if range.length == 0 {
            return;
        }
        let rows = NSIndexSet::indexSetWithIndexesInRange(range);
        let cols = NSIndexSet::indexSetWithIndexesInRange(NSRange::new(0, tv.numberOfColumns() as usize));
        tv.reloadDataForRowIndexes_columnIndexes(&rows, &cols);
    }

    /// Reloads every row (rows came, went or moved) and selects the same ids again.
    pub fn reload_keep_selection(&self) {
        let Some(tv) = self.table() else { return };
        let ids = self.selected();
        self.ivars().restoring.set(true);
        tv.reloadData();
        let rows = if ids.is_empty() {
            Vec::new()
        } else if let Some(ov) =
            tv.downcast_ref::<NSOutlineView>().filter(|_| self.ivars().hooks.children.is_some())
        {
            // SAFETY: the items are the NSNumbers this source hands the outline.
            ids.iter()
                .filter_map(|&id| usize::try_from(unsafe { ov.rowForItem(Some(&self.item(id))) }).ok())
                .collect()
        } else {
            (self.ivars().hooks.rows_of)(&ids)
        };
        tv.selectRowIndexes_byExtendingSelection(&index_set(rows), false);
        self.ivars().restoring.set(false);
        // ids gone from the list leave the selection
        let now = rows_of_set(&tv.selectedRowIndexes());
        if now.len() != ids.len() {
            self.selection_did_change();
        }
    }

    /// Selects these ids (none: clears the selection), as the user would.
    pub fn select(&self, ids: &[u64]) {
        let Some(tv) = self.table() else { return };
        let rows = (self.ivars().hooks.rows_of)(ids);
        tv.selectRowIndexes_byExtendingSelection(&index_set(rows), false);
    }

    /// Shows or hides the column with spec index `i`.
    pub fn set_column_visible(&self, i: usize, visible: bool) {
        if let Some(c) = self.ivars().columns.borrow().get(i) {
            c.setHidden(!visible);
        }
    }
}

/// The indices in an index set.
pub fn rows_of_set(set: &NSIndexSet) -> Vec<usize> {
    let mut out = Vec::with_capacity(set.count());
    let mut i = set.firstIndex();
    while i != NSNotFound as usize {
        out.push(i);
        i = set.indexGreaterThanIndex(i);
    }
    out
}

/// An index set of `rows`, in ranges (a ⌘A of 100,000 rows is one range).
pub fn index_set(mut rows: Vec<usize>) -> Retained<NSIndexSet> {
    rows.sort_unstable();
    rows.dedup();
    let set = NSMutableIndexSet::new();
    let mut i = 0;
    while i < rows.len() {
        let start = rows[i];
        let mut end = start;
        while i + 1 < rows.len() && rows[i + 1] == end + 1 {
            i += 1;
            end += 1;
        }
        set.addIndexesInRange(NSRange::new(start, end - start + 1));
        i += 1;
    }
    Retained::into_super(set)
}

fn text_cell(spec: &ColumnSpec, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let t = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    t.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    t.setAlignment(match spec.align {
        Align::Left => NSTextAlignment::Natural,
        Align::Right => NSTextAlignment::Right,
        Align::Center => NSTextAlignment::Center,
    });
    auto(&t);
    t
}

/// A new cell view for `spec`'s column.
fn make_cell(spec: &ColumnSpec, mtm: MainThreadMarker) -> Retained<NSView> {
    use crate::ui::layout::activate;
    if spec.kind == CellKind::Bar {
        return Retained::into_super(RPCanvas::new(
            canvas::Kind::DoneBar,
            Box::new(canvas::Bar::default()),
            mtm,
        ));
    }
    if let CellKind::Drawn(kind) = spec.kind {
        return Retained::into_super(RPCanvas::new(kind, Box::new(()), mtm));
    }
    let cell = NSTableCellView::new(mtm);
    if spec.kind == CellKind::Check {
        // SAFETY: the target and action are set by the caller (`cell_view`); nil for now.
        let b =
            unsafe { NSButton::checkboxWithTitle_target_action(&NSString::from_str(""), None, None, mtm) };
        b.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        auto(&b);
        cell.addSubview(&b);
        crate::ui::layout::activate(&[
            b.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 2.0),
            b.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&cell.trailingAnchor(), -2.0),
            b.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
        ]);
        return Retained::into_super(cell);
    }
    let text = text_cell(spec, mtm);
    cell.addSubview(&text);
    let leading = if spec.kind == CellKind::IconText {
        let image = NSImageView::new(mtm);
        auto(&image);
        cell.addSubview(&image);
        activate(&[
            image.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 2.0),
            image.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
            image.widthAnchor().constraintEqualToConstant(16.0),
            image.heightAnchor().constraintEqualToConstant(16.0),
        ]);
        // SAFETY: the image view is the cell's subview and lives with it.
        unsafe { cell.setImageView(Some(&image)) };
        image.trailingAnchor().constraintEqualToAnchor_constant(&text.leadingAnchor(), -4.0)
    } else {
        cell.leadingAnchor().constraintEqualToAnchor_constant(&text.leadingAnchor(), -2.0)
    };
    activate(&[
        leading,
        text.trailingAnchor().constraintEqualToAnchor_constant(&cell.trailingAnchor(), -2.0),
        text.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
    ]);
    // SAFETY: the text field is the cell's subview and lives with it.
    unsafe { cell.setTextField(Some(&text)) };
    Retained::into_super(cell)
}

fn fill_cell(view: &NSView, value: CellValue) {
    if let Some(c) = view.downcast_ref::<RPCanvas>() {
        match value {
            CellValue::Bar(bar) => {
                c.set_accessibility("", &bar.text);
                c.set_value(Box::new(bar));
            }
            CellValue::Drawn(v, text) => {
                c.set_accessibility("", &text);
                c.set_value(v);
            }
            _ => {}
        }
        return;
    }
    if let Some(b) = check_box(view) {
        if let CellValue::Check(on, title) = value {
            b.setTitle(&NSString::from_str(&title));
            b.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        }
        return;
    }
    let Some(cell) = view.downcast_ref::<NSTableCellView>() else { return };
    let (image, text) = match value {
        CellValue::Text(t) => (None, t),
        CellValue::IconText(i, t) => (i, t),
        CellValue::Bar(b) => (None, b.text),
        CellValue::Drawn(_, t) => (None, t),
        CellValue::Check(_, t) => (None, t),
        CellValue::Empty => (None, String::new()),
    };
    // SAFETY: the cell's own subviews, set in `make_cell`.
    if let Some(t) = unsafe { cell.textField() } {
        t.setStringValue(&NSString::from_str(&text));
    }
    // SAFETY: as above.
    if let Some(iv) = unsafe { cell.imageView() } {
        iv.setImage(image.as_deref());
    }
}

/// The check box of a `CellKind::Check` cell view.
fn check_box(view: &NSView) -> Option<Retained<NSButton>> {
    view.downcast_ref::<NSTableCellView>()?.subviews().firstObject()?.downcast::<NSButton>().ok()
}

/// A table of `specs` with `hooks`, in a scroll view; the source must be kept (tables hold
/// their data source and delegate weakly).
pub fn table(
    kind: TableKind,
    specs: Vec<ColumnSpec>,
    hooks: Hooks,
    sort: Option<(usize, bool)>,
    mtm: MainThreadMarker,
) -> (Retained<objc2_app_kit::NSScrollView>, Retained<RPTableView>, Retained<RPTableSource>) {
    let tv = RPTableView::make(mtm);
    let source = build(&tv, kind, specs, hooks, mtm);
    if let Some((i, ascending)) = sort {
        if let Some(s) = source.ivars().specs.get(i).filter(|s| s.sortable) {
            let d = NSSortDescriptor::sortDescriptorWithKey_ascending(
                Some(&NSString::from_str(&s.id)),
                ascending,
            );
            tv.setSortDescriptors(&NSArray::from_retained_slice(&[d]));
        }
    }
    let scroll = scroll(&tv, mtm);
    (scroll, tv, source)
}

/// An outline (a source-list sidebar when `source_list`) of `specs` with tree `hooks`
/// (`hooks.children` set).
pub fn outline(
    kind: TableKind,
    specs: Vec<ColumnSpec>,
    hooks: Hooks,
    source_list: bool,
    mtm: MainThreadMarker,
) -> (Retained<objc2_app_kit::NSScrollView>, Retained<NSOutlineView>, Retained<RPTableSource>) {
    let ov = NSOutlineView::new(mtm);
    let source = build(&ov, kind, specs, hooks, mtm);
    if let Some(c) = source.ivars().columns.borrow().first() {
        // SAFETY: the column is the outline's own, kept by the source.
        unsafe { ov.setOutlineTableColumn(Some(c)) };
    }
    ov.setHeaderView(None);
    if source_list {
        ov.setStyle(NSTableViewStyle::SourceList);
        ov.setFloatsGroupRows(false);
    }
    // SAFETY: the source is the outline's data source and delegate; the caller keeps it.
    unsafe {
        ov.setDataSource(Some(ProtocolObject::from_ref(&*source)));
        ov.setDelegate(Some(ProtocolObject::from_ref(&*source)));
    }
    ov.reloadData();
    // SAFETY: nil expands every top-level item.
    unsafe { ov.expandItem_expandChildren(None, true) };
    let scroll = scroll(&ov, mtm);
    scroll.setHasHorizontalScroller(false);
    ov.setColumnAutoresizingStyle(
        objc2_app_kit::NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle,
    );
    ov.sizeLastColumnToFit();
    (scroll, ov, source)
}

fn scroll(tv: &NSTableView, mtm: MainThreadMarker) -> Retained<objc2_app_kit::NSScrollView> {
    let s = objc2_app_kit::NSScrollView::new(mtm);
    s.setDocumentView(Some(tv));
    s.setHasVerticalScroller(true);
    s.setHasHorizontalScroller(true);
    s.setAutohidesScrollers(true);
    auto(&s);
    s
}

/// Columns, header menu, data source and delegate of `tv`.
fn build(
    tv: &NSTableView,
    kind: TableKind,
    specs: Vec<ColumnSpec>,
    hooks: Hooks,
    mtm: MainThreadMarker,
) -> Retained<RPTableSource> {
    let is_tree = hooks.children.is_some();
    let source = RPTableSource::new(kind, specs, hooks, mtm);
    *source.ivars().view.borrow_mut() = Weak::from(tv);
    let header_menu = NSMenu::new(mtm);
    let mut columns = Vec::new();
    for (i, s) in source.ivars().specs.iter().enumerate() {
        let c = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &NSString::from_str(&s.id));
        c.setTitle(&NSString::from_str(&s.title));
        c.setWidth(s.width);
        c.setMinWidth(s.min_width);
        c.setHidden(!s.visible);
        if s.align == Align::Right {
            c.headerCell().setAlignment(NSTextAlignment::Right);
        }
        if s.sortable {
            let d = NSSortDescriptor::sortDescriptorWithKey_ascending(
                Some(&NSString::from_str(&s.id)),
                !s.descending_first,
            );
            c.setSortDescriptorPrototype(Some(&d));
        }
        tv.addTableColumn(&c);
        if s.hideable {
            // SAFETY: `toggleColumn:` is RPTableSource's action; the source is the target and
            // outlives the menu (both live as long as the table's owner keeps them).
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&s.title),
                    Some(sel!(toggleColumn:)),
                    &NSString::from_str(""),
                )
            };
            item.setTag(i as isize);
            // SAFETY: as above.
            unsafe { item.setTarget(Some(&source)) };
            header_menu.addItem(&item);
        }
        columns.push(c);
    }
    *source.ivars().columns.borrow_mut() = columns;
    if let Some(h) = tv.headerView() {
        // SAFETY: the header keeps its menu.
        unsafe { h.setMenu(Some(&header_menu)) };
    }
    tv.setAllowsMultipleSelection(!is_tree);
    tv.setAllowsColumnReordering(!is_tree);
    tv.setRowHeight(18.0);
    if !is_tree {
        tv.setStyle(NSTableViewStyle::FullWidth);
        tv.setUsesAlternatingRowBackgroundColors(true);
        // SAFETY: the source is the table's data source and delegate; the caller keeps it.
        unsafe {
            tv.setDataSource(Some(ProtocolObject::from_ref(&*source)));
            tv.setDelegate(Some(ProtocolObject::from_ref(&*source)));
        }
    }
    // SAFETY: `doubleClick:` is the source's action; the table keeps its target weakly.
    unsafe {
        tv.setTarget(Some(&source));
        tv.setDoubleAction(Some(sel!(doubleClick:)));
    }
    source
}

define_class!(
    // SAFETY: NSTableView may be subclassed; RPTableView adds no ivars and does not implement Drop.
    #[unsafe(super(NSTableView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RPTableView"]
    pub struct RPTableView;

    impl RPTableView {
        /// Return opens, ⌫ removes (through the source's `key` hook); other keys as usual.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let key = match event.keyCode() {
                36 | 76 => Some(Key::Return),
                51 | 117 => Some(Key::Delete),
                _ => None,
            };
            let handled = match (key, self.source()) {
                (Some(k), Some(s)) => match &s.ivars().hooks.key {
                    Some(f) => f(k, &s.selected()),
                    None => false,
                },
                _ => false,
            };
            if !handled {
                // SAFETY: NSTableView's own keyDown:, with the same event.
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }

        /// The context menu of the row under the mouse: it joins the selection first.
        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> Option<Retained<NSMenu>> {
            self.context_menu(event)
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            // SAFETY: NSView's own method.
            let _: () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            // the state icons have a dark palette
            if let Some(s) = self.source() {
                s.reload_visible();
            }
        }
    }
);

impl RPTableView {
    fn context_menu(&self, event: &NSEvent) -> Option<Retained<NSMenu>> {
        let source = self.source()?;
        let f = source.ivars().hooks.menu.as_ref()?;
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        let row = self.rowAtPoint(p);
        if row >= 0 && !self.selectedRowIndexes().containsIndex(row as usize) {
            self.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row as usize), false);
        }
        f(&source.selected())
    }

    fn make(mtm: MainThreadMarker) -> Retained<RPTableView> {
        let this = RPTableView::alloc(mtm).set_ivars(());
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 200.0));
        // SAFETY: NSTableView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn source(&self) -> Option<Retained<RPTableSource>> {
        // SAFETY: `delegate` is NSTableView's property; ours is always an RPTableSource.
        let d: Option<Retained<AnyObject>> = unsafe { msg_send![self, delegate] };
        d.and_then(|d| d.downcast::<RPTableSource>().ok())
    }
}
