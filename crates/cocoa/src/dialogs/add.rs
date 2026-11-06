//! Add New Torrent (µTorrent 2.2.1, as the GTK and Win32 dialogs): Save In with the recent
//! folders and a folder panel, Name, the options and Label on the left; the torrent's contents
//! with a file tree of check boxes and priorities on the right; "Don't show this again" under
//! them. One window per torrent, not modal: several can be open, each answering its own
//! [`AddRequest`] through `Core::finish_add` (a magnet's next dialog follows, one at a time).
//!
//! The file tree is an `NSOutlineView` (`table::outline`) over the frontend's [`FileTree`] by
//! node. A click on a row's check box (the image before its name) checks or unchecks it; a double
//! click opens or closes a folder and checks or unchecks a file; the context menu sets the
//! priority, checks or unchecks the selected rows (or the clicked one).

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplication, NSBorderType, NSButton, NSComboBox, NSEvent, NSImage, NSLayoutAttribute,
    NSLayoutConstraintOrientation, NSOutlineView, NSStackViewDistribution, NSTableCellView,
    NSTableHeaderView, NSTextField, NSView, NSWindow,
};
use objc2_foundation::{NSFileManager, NSFileSystemFreeSize, NSNumber, NSSize};
use rookparcel_frontend::add::{self as fadd, AddChoice, Check, FileTree};
use rookparcel_frontend::app::{AddRequest, AddStep};
use rookparcel_frontend::detail::{PRIORITIES, priority_text};
use rookparcel_frontend::{fmt, lang, tr};

use super::{NOTHING, Observer, cmd};
use crate::app::{apply, peek, with_core};
use crate::menu::{self, Item};
use crate::table::{self, Align, CellKind, CellValue, ColumnSpec, Hooks, RPTableSource, TableKind};
use crate::ui::{self, layout, ns};

// the dialog's actions
const OK: u8 = 1;
const CANCEL: u8 = 2;
const CLOSED: u8 = 3;
const BROWSE: u8 = 4;
const SAVE_IN: u8 = 5;
const ALL: u8 = 6;
const NONE: u8 = 7;
const CLICK: u8 = 8;
const CHECK: u8 = 9;
const UNCHECK: u8 = 10;
/// `PRIORITY + i`: `PRIORITIES[i]`.
const PRIORITY: u8 = 16;

/// The dialog's size when none is remembered.
const SIZE: (f64, f64) = (840.0, 520.0);

/// File > Add Torrent: an open panel for `.torrent` files (several at once), each through
/// `Core::add_file`.
pub fn open_files() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let pick = ui::Pick {
        files: true,
        multiple: true,
        extensions: vec!["torrent"],
        message: ui::text(tr!(PickTorrent)),
        ..ui::Pick::default()
    };
    let main = super::main_window();
    super::open_panel(
        main.as_deref(),
        pick,
        |paths| {
            for p in paths {
                add_path(&p);
            }
        },
        mtm,
    );
}

/// A `.torrent` file on disk, through `Core::add_file` (the dialog, a message, or added).
pub fn add_path(path: &Path) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    match std::fs::read(path) {
        Ok(bytes) => {
            if let Some(step) = with_core(|c| c.add_file(bytes)) {
                handle(step);
            }
        }
        Err(e) => {
            super::tell(None, ui::Style::Error, &tr!(CannotOpen, path = path.display()), &e.to_string(), mtm)
        }
    }
}

/// What an add (`Core::add_file`, `add_magnet`, `add_url`, `add_link`) asks of the GUI: the Add
/// New Torrent dialog for `AddStep::Dialog`, a message for `AlreadyInList` and `Error`, nothing
/// for the others. Slice C calls it for dropped files, slice G for opened files and links.
pub fn handle(step: AddStep) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    match step {
        AddStep::Dialog(r) => show(r, mtm),
        AddStep::AlreadyInList(name) => {
            super::tell(None, ui::Style::Info, &tr!(InfoAlreadyInList, name), "", mtm)
        }
        AddStep::Error(message, detail) => super::tell(None, ui::Style::Error, &message, &detail, mtm),
        AddStep::Added | AddStep::Awaiting | AddStep::Fetching | AddStep::Nothing => {}
    }
}

/// `Events::magnet_ready` (open `Core::next_magnet_dialog`, one at a time) and `Events::adds`
/// (dialogs for downloaded `.torrent` files), from `app::apply`.
pub fn from_events(magnet_ready: bool, adds: Vec<AddRequest>) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    for r in adds {
        show(r, mtm);
    }
    if magnet_ready {
        if let Some(Some(r)) = with_core(|c| c.next_magnet_dialog()) {
            show(r, mtm);
        }
    }
}

/// One open dialog; its handler owns it.
struct Dialog {
    id: u16,
    /// The request, until OK or Cancel answers it.
    req: RefCell<Option<AddRequest>>,
    tree: Rc<RefCell<FileTree>>,
    window: Retained<NSWindow>,
    outline: Retained<NSOutlineView>,
    /// The outline's data source (outlines keep theirs weakly).
    files: Retained<RPTableSource>,
    save: Retained<NSComboBox>,
    name: Retained<NSTextField>,
    skip: Retained<NSButton>,
    start: Retained<NSButton>,
    top: Retained<NSButton>,
    label: Retained<NSComboBox>,
    again: Retained<NSButton>,
    size: Retained<NSTextField>,
    in_session: bool,
    observer: RefCell<Option<Observer>>,
    /// A click's check box toggle, so the double click that follows does not undo it.
    toggled_by_click: Cell<bool>,
}

/// An editable combo box of `items` (the Save In folders, the labels).
fn combo(
    items: &[String],
    value: &str,
    action: crate::commands::Command,
    mtm: MainThreadMarker,
) -> Retained<NSComboBox> {
    let c = NSComboBox::new(mtm);
    for i in items {
        // SAFETY: a combo box's items are strings here (no data source).
        unsafe { c.addItemWithObjectValue(&ns(i)) };
    }
    c.setNumberOfVisibleItems(10);
    c.setCompletes(true);
    c.setStringValue(&ns(value));
    ui::target::wire(&c, action, mtm);
    ui::auto(&c);
    c
}

/// A value of the Torrent Contents part: one line, selectable, cut with "…", whole in its tooltip.
fn value_label(s: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = ui::truncating_label(s, mtm);
    l.setSelectable(true);
    ui::tooltip(&l, s);
    layout::stretch(&l);
    l
}

/// The check box images: SF Symbols (macOS 11).
fn check_image(c: Check) -> Option<Retained<NSImage>> {
    let (name, description) = match c {
        Check::On => ("checkmark.square.fill", tr!(MenuCheck)),
        Check::Off => ("square", tr!(MenuUncheck)),
        Check::Mixed => ("minus.square.fill", tr!(AddMixed)),
    };
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), Some(&ns(description)))
}

fn show(req: AddRequest, mtm: MainThreadMarker) {
    let id = super::new_id();
    let remote = peek(|c| c.is_remote()).unwrap_or(false);
    let ti = req.ti.clone();
    let setup = req.setup.clone();

    // Save In, Name, the options and Label
    let save =
        combo(&setup.save_dirs, setup.save_dirs.first().map_or("", String::as_str), cmd(id, SAVE_IN), mtm);
    layout::stretch(&save);
    layout::min_width(&save, 200.0);
    let browse = ui::button("\u{2026}", cmd(id, BROWSE), mtm);
    ui::tooltip(&browse, &ui::text(tr!(AddPickFolder)));
    // a folder on this computer means nothing to a remote server: the path is typed
    browse.setEnabled(!remote);
    let save_row = layout::hstack(&[&save, &browse], mtm);
    let name = ui::field(ti.name(), "", mtm);
    let check = |s: &str, on: bool| ui::check(&ui::text(s), on, cmd(id, NOTHING), mtm);
    let skip = check(tr!(AddSkipCheck), false);
    skip.setEnabled(!setup.in_session);
    let start = check(tr!(AddStart), setup.start);
    let top = check(tr!(AddTopOfQueue), false);
    let label = combo(&setup.labels, "", cmd(id, NOTHING), mtm);
    layout::stretch(&label);
    let label_row = layout::hstack(&[&ui::label_tr(tr!(AddLabel), mtm), &label], mtm);
    let options = super::column(&[&skip, &start, &top, &label_row], mtm);
    // the column is as tall as the contents beside it: the room left goes under its controls
    let rest = NSView::new(mtm);
    ui::auto(&rest);
    rest.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    let left = super::column(
        &[
            &super::section(tr!(AddSaveIn), &save_row, mtm),
            &super::section(tr!(AddName), &name, mtm),
            &super::section(tr!(AddOptions), &options, mtm),
            &rest,
        ],
        mtm,
    );
    left.setSpacing(16.0);
    left.setDistribution(NSStackViewDistribution::Fill);
    layout::width(&left, 260.0);

    // Torrent Contents: the torrent's facts, the files, Select All / None
    let size = value_label("", mtm);
    let date = ti.creation_date().map(fmt::date_time).unwrap_or_default();
    let caption = |s: &str| ui::label_tr(s, mtm);
    let (n1, n2, n3, n4) = (
        caption(tr!(AddTorrentName)),
        caption(tr!(AddComment)),
        caption(tr!(AddSize)),
        caption(tr!(AddDate)),
    );
    let (v1, v2, v4) = (value_label(ti.name(), mtm), value_label(ti.comment(), mtm), value_label(&date, mtm));
    let info = layout::grid(
        &[
            vec![Some(&*n1), Some(&*v1)],
            vec![Some(&*n2), Some(&*v2)],
            vec![Some(&*n3), Some(&*size)],
            vec![Some(&*n4), Some(&*v4)],
        ],
        mtm,
    );
    let tree = Rc::new(RefCell::new(FileTree::from_torrent(&ti)));
    let (scroll, outline, source) = files_outline(&tree, id, mtm);
    layout::activate(&[scroll.heightAnchor().constraintGreaterThanOrEqualToConstant(160.0)]);
    let select = layout::hstack(
        &[
            &ui::button_tr(tr!(MenuSelectAll), cmd(id, ALL), mtm),
            &ui::button_tr(tr!(MenuSelectNone), cmd(id, NONE), mtm),
        ],
        mtm,
    );
    let contents = super::column(&[&info, &scroll, &select], mtm);
    // the buttons keep their size at the leading edge
    select.setAlignment(NSLayoutAttribute::CenterY);
    let right = super::section(tr!(AddContents), &contents, mtm);
    layout::stretch(&right);

    let body = layout::hstack(&[&left, &right], mtm);
    body.setAlignment(NSLayoutAttribute::Top);
    // the contents take the width the left column leaves
    body.setDistribution(NSStackViewDistribution::Fill);
    body.setSpacing(20.0);
    layout::activate(&[right.bottomAnchor().constraintEqualToAnchor(&body.bottomAnchor())]);
    let again = check(tr!(AddDontShowAgain), false);
    let ok = ui::button_tr(tr!(BtnOk), cmd(id, OK), mtm);
    let cancel = ui::button_tr(tr!(BtnCancel), cmd(id, CANCEL), mtm);
    let buttons = super::buttons(&[&again], &cancel, &ok, mtm);
    let root = super::column(&[&body, &buttons], mtm);
    root.setSpacing(16.0);
    body.setContentHuggingPriority_forOrientation(
        1.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Vertical,
    );

    let (w, h) = setup.window.map_or(SIZE, |(w, h)| (f64::from(w), f64::from(h)));
    let window = super::window(&ui::text(tr!(AddTitle)), &root, Some(NSSize::new(w, h)), true, mtm);
    window.setInitialFirstResponder(Some(&ok));

    let d = Rc::new(Dialog {
        id,
        req: RefCell::new(Some(req)),
        tree,
        window: window.clone(),
        outline,
        files: source,
        save,
        name,
        skip,
        start,
        top,
        label,
        again,
        size,
        in_session: setup.in_session,
        observer: RefCell::new(None),
        toggled_by_click: Cell::new(false),
    });
    *d.observer.borrow_mut() = Some(super::on_close(&window, id, CLOSED));
    let handler = d.clone();
    super::set_handler(id, move |act| handler.act(act));
    super::test_accept(id, OK);
    d.update_size();
    window.center();
    window.makeKeyAndOrderFront(None);
}

/// The file tree: Name (check box and name), Size, Priority, over the tree's nodes (an item's id
/// is its node + 1).
fn files_outline(
    tree: &Rc<RefCell<FileTree>>,
    id: u16,
    mtm: MainThreadMarker,
) -> (Retained<objc2_app_kit::NSScrollView>, Retained<NSOutlineView>, Retained<RPTableSource>) {
    // the Name column takes the width the others leave (the outline's first-column autoresizing)
    let mut name = ColumnSpec::text("name", &ui::text(tr!(ColName)), 120.0);
    name.kind = CellKind::IconText;
    name.hideable = false;
    let mut size = ColumnSpec::text("size", &ui::text(tr!(ColSize)), 72.0);
    size.align = Align::Right;
    let priority = ColumnSpec::text("priority", &ui::text(tr!(ColPriority)), 96.0);

    let t = tree.clone();
    let mut hooks = Hooks::flat(
        || 0,
        move |item, col, _dark| {
            let Ok(tree) = t.try_borrow() else { return CellValue::Empty };
            let Some(row) = item.checked_sub(1).and_then(|n| tree.node(n as usize)) else {
                return CellValue::Empty;
            };
            match col {
                0 => CellValue::IconText(check_image(row.check), row.name.to_owned()),
                1 => CellValue::Text(fmt::size(row.size)),
                _ => CellValue::Text(row.priority.map_or(tr!(AddMixed), priority_text).to_owned()),
            }
        },
    );
    let t = tree.clone();
    hooks.children = Some(Box::new(move |parent| {
        let Ok(tree) = t.try_borrow() else { return Vec::new() };
        let parent = match parent {
            None => None,
            Some(p) => match p.checked_sub(1) {
                Some(n) => Some(n as usize),
                None => return Vec::new(),
            },
        };
        tree.children(parent).into_iter().map(|n| n as u64 + 1).collect()
    }));
    let widths: Vec<f64> = [&name, &size, &priority].iter().map(|c| c.width).collect();
    let (scroll, outline, source) =
        table::outline(TableKind("add-files"), vec![name, size, priority], hooks, false, mtm);
    // the outline sized its last column to its first (empty) frame: the widths again, and
    // from now on only Name follows the outline's width
    for (c, w) in outline.tableColumns().iter().zip(widths) {
        c.setWidth(w);
    }
    outline.setHeaderView(Some(&NSTableHeaderView::new(mtm)));
    outline.setAllowsMultipleSelection(true);
    outline.setUsesAlternatingRowBackgroundColors(true);
    scroll.setBorderType(NSBorderType::BezelBorder);
    // clicks come to the dialog: its check boxes, folders opening on a double click
    ui::target::wire(&outline, cmd(id, CLICK), mtm);
    // SAFETY: `perform:` is RPTarget's action, the target `wire` set.
    unsafe { outline.setDoubleAction(Some(ui::target::perform())) };
    let mut items: Vec<Item> = PRIORITIES
        .iter()
        .enumerate()
        .map(|(i, (s, _))| Item::Cmd {
            cmd: cmd(id, PRIORITY + i as u8),
            title: ui::text(lang::text(*s)),
            key: None,
        })
        .collect();
    items.push(Item::Sep);
    items.push(Item::Cmd { cmd: cmd(id, CHECK), title: ui::text(tr!(MenuCheck)), key: None });
    items.push(Item::Cmd { cmd: cmd(id, UNCHECK), title: ui::text(tr!(MenuUncheck)), key: None });
    // SAFETY: the outline keeps its menu; the items' target is the shared RPTarget.
    unsafe { outline.setMenu(Some(&menu::build("", &items, mtm))) };
    (scroll, outline, source)
}

impl Dialog {
    fn act(self: &Rc<Self>, act: u8) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        match act {
            OK => self.ok(mtm),
            CANCEL => self.answer(None, false, true),
            CLOSED => self.answer(None, false, false),
            BROWSE => self.browse(mtm),
            SAVE_IN => self.update_size(),
            ALL | NONE => {
                self.tree.borrow_mut().set_all(act == ALL);
                self.changed();
            }
            CLICK => self.click(mtm),
            CHECK | UNCHECK => {
                let nodes = self.targets();
                self.tree.borrow_mut().set_node_checks(&nodes, act == CHECK);
                self.changed();
            }
            a if a >= PRIORITY => {
                let Some(&(_, p)) = PRIORITIES.get(usize::from(a - PRIORITY)) else { return };
                let nodes = self.targets();
                self.tree.borrow_mut().set_node_priority(&nodes, p);
                self.changed();
            }
            _ => {}
        }
    }

    /// OK: the choices, or a word about the Save In folder.
    fn ok(&self, mtm: MainThreadMarker) {
        let save_path = match fadd::save_folder(&ui::value(&self.save)) {
            Ok(dir) => dir,
            Err(e) => {
                super::tell(Some(&self.window), ui::Style::Warning, &e, "", mtm);
                self.window.makeFirstResponder(Some(&self.save));
                return;
            }
        };
        let torrent_name = match self.req.borrow().as_ref() {
            Some(r) => r.name().to_owned(),
            None => return,
        };
        let new_name = ui::value(&self.name).trim().to_owned();
        let choice = AddChoice {
            save_path,
            name: (!new_name.is_empty() && new_name != torrent_name).then_some(new_name),
            file_priorities: self.tree.borrow().file_priorities(),
            start: ui::checked(&self.start),
            skip_hash_check: !self.in_session && ui::checked(&self.skip),
            top_of_queue: ui::checked(&self.top),
            label: ui::value(&self.label).trim().to_owned(),
        };
        self.answer(Some(choice), ui::checked(&self.again), true);
    }

    /// OK (`choice`), Cancel or the window's close: the request goes to `Core::finish_add` once,
    /// the window closes (unless it is closing already), and a magnet's next dialog opens.
    fn answer(&self, choice: Option<AddChoice>, dont_show_again: bool, close: bool) {
        let Some(req) = self.req.borrow_mut().take() else { return };
        let content = self.window.contentLayoutRect().size;
        let size = (content.width.round() as i32, content.height.round() as i32);
        if let Some(o) = self.observer.borrow_mut().take() {
            super::forget(&o);
        }
        if close {
            self.window.close();
        }
        super::retire(self.id);
        let done = with_core(|c| {
            fadd::remember_dialog(c.prefs_mut(), size, dont_show_again);
            let next = c.finish_add(req, choice);
            c.save_prefs();
            (next, c.events())
        });
        let Some((next, events)) = done else { return };
        apply(Some(events));
        if let (Some(r), Some(mtm)) = (next, MainThreadMarker::new()) {
            show(r, mtm);
        }
    }

    /// Save In's "…": the folder panel.
    fn browse(self: &Rc<Self>, mtm: MainThreadMarker) {
        let now = ui::value(&self.save);
        let pick = ui::Pick {
            folders: true,
            start: Some(Path::new(now.trim()).to_path_buf()).filter(|p| p.is_dir()),
            message: ui::text(tr!(AddPickFolder)),
            ..ui::Pick::default()
        };
        let me: Weak<Dialog> = Rc::downgrade(self);
        super::open_panel(
            Some(&self.window),
            pick,
            move |paths| {
                let (Some(d), Some(p)) = (me.upgrade(), paths.first()) else { return };
                d.save.setStringValue(&ns(&p.to_string_lossy()));
                d.update_size();
            },
            mtm,
        );
    }

    /// The node in outline row `row`.
    fn node_at(&self, row: isize) -> Option<usize> {
        let item = self.outline.itemAtRow(row)?;
        let id = item.downcast_ref::<NSNumber>()?.as_u64();
        id.checked_sub(1).map(|n| n as usize)
    }

    /// What the context menu acts on: the selected rows, or the clicked row when it is not one of
    /// them.
    fn targets(&self) -> Vec<usize> {
        let clicked = self.outline.clickedRow();
        let selected = table::rows_of_set(&self.outline.selectedRowIndexes());
        let rows = match usize::try_from(clicked) {
            Ok(c) if !selected.contains(&c) => vec![c],
            _ => selected,
        };
        rows.into_iter().filter_map(|r| self.node_at(r as isize)).collect()
    }

    /// A click in the tree: on a check box it toggles the row; a double click opens or closes a
    /// folder, and toggles a file (unless its first click did).
    fn click(&self, mtm: MainThreadMarker) {
        let row = self.outline.clickedRow();
        let Some(node) = self.node_at(row) else { return };
        let event = NSApplication::sharedApplication(mtm).currentEvent();
        let double = event.as_ref().is_some_and(|e| e.clickCount() >= 2);
        let on_box =
            self.outline.clickedColumn() == 0 && event.as_ref().is_some_and(|e| self.on_check_box(row, e));
        if !double {
            self.toggled_by_click.set(on_box);
            if on_box {
                self.toggle(node);
            }
            return;
        }
        let folder = self.tree.borrow().node(node).is_some_and(|r| r.folder.is_some());
        if folder {
            let Some(item) = self.outline.itemAtRow(row) else { return };
            let item: &AnyObject = &item;
            // SAFETY: the item is one of the outline's own (from `itemAtRow:`).
            unsafe {
                if self.outline.isItemExpanded(Some(item)) {
                    self.outline.collapseItem(Some(item));
                } else {
                    self.outline.expandItem(Some(item));
                }
            }
        } else if !self.toggled_by_click.get() {
            self.toggle(node);
        }
        self.toggled_by_click.set(false);
    }

    /// The event's point is on the check box image of row `row` (a few points around it count).
    fn on_check_box(&self, row: isize, event: &NSEvent) -> bool {
        let Some(view) = self.outline.viewAtColumn_row_makeIfNecessary(0, row, false) else { return false };
        let Some(cell) = view.downcast_ref::<NSTableCellView>() else { return false };
        // SAFETY: the cell's own image view, set when the cell was made.
        let Some(image) = (unsafe { cell.imageView() }) else { return false };
        let f = image.frame();
        let p = cell.convertPoint_fromView(event.locationInWindow(), None);
        p.x >= f.origin.x - 4.0 && p.x <= f.origin.x + f.size.width + 4.0
    }

    fn toggle(&self, node: usize) {
        self.tree.borrow_mut().toggle_node_checks(&[node]);
        self.changed();
    }

    /// After check or priority changes: the rows on screen and the size.
    fn changed(&self) {
        self.files.reload_visible();
        self.update_size();
    }

    /// "1.20 GB of 3.40 GB selected (disk space: 120 GB free)".
    fn update_size(&self) {
        let (selected, all) = self.tree.borrow().sizes();
        let mut text = fadd::size_text(selected, all);
        if let Some(free) = free_space(&ui::value(&self.save)) {
            text.push(' ');
            text.push_str(&tr!(AddDiskSpace, free = fmt::size(free)));
        }
        self.size.setStringValue(&ns(&text));
    }
}

/// Free bytes on the volume of `dir` (or of its nearest existing parent).
fn free_space(dir: &str) -> Option<u64> {
    let mut p = Path::new(dir.trim());
    loop {
        if p.as_os_str().is_empty() {
            return None;
        }
        if p.is_dir() {
            let attrs = NSFileManager::defaultManager()
                .attributesOfFileSystemForPath_error(&ns(&p.to_string_lossy()))
                .ok()?;
            // SAFETY: the attribute key constant.
            let free = attrs.objectForKey(unsafe { NSFileSystemFreeSize })?;
            return free.downcast_ref::<NSNumber>().map(|n| n.as_u64());
        }
        p = p.parent()?;
    }
}
