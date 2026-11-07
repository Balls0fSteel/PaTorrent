//! The Files tab: the torrent's files (`FILE_COLUMNS`) under the D50 filter field at the tab's
//! top right, with "N of M files" beside it while it hides some (the field's own button clears
//! it). By default they show in their folders (D62): an outline over `Detail::file_tree`'s
//! nodes, as the Add dialog's tree (an item's id is its node + 1; AppKit keeps which folders are
//! open, by item, over reloads). The Tree View check box beside the field, and the menu's Tree
//! View, switch to 2.2.1's plain list of paths and back; the choice is kept (`ui.files_tree`).
//!
//! Its menu, as µTorrent 2.2.1's (but for Scan for Viruses): Open and Open Containing Folder
//! when the files are on this Mac; Prioritize by File Order and Priority ▸; Don't Download;
//! Relocate (one file, on this Mac); Tree View. In the tree a folder stands for its (shown)
//! files, and Open and Open Containing Folder take the folder itself. A double click opens a
//! file (in the tree, it opens or closes a folder); Return opens a file in the plain list; ⌘A
//! selects the files the filter shows (the table's own Select All); ⌘F in the tab goes to the
//! field.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSButton, NSColor, NSFont, NSOutlineView, NSScrollView, NSSearchField, NSStackView, NSTextField, NSView,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSNumber, NSString, NSURL};
use rookparcel_core::TorrentId;
use rookparcel_frontend::detail::{FILES_TREE_KEY, PRIORITIES, Tab, priority_text};
use rookparcel_frontend::tr;

use super::tables::{self, Table};
use crate::app::{apply, peek, with_core};
use crate::commands::Command;
use crate::menu::{self, Item, Role};
use crate::table::{self, CellValue, Hooks, Key, RPTableSource, TableKind};
use crate::ui::{self, layout};

/// The tab's views.
pub struct Files {
    pub view: Retained<NSView>,
    pub filter: Retained<NSSearchField>,
    count: Retained<NSTextField>,
    /// Tree View: the folder tree (on) or the plain list.
    toggle: Retained<NSButton>,
    /// The filter's row, above the list.
    bar: Retained<NSStackView>,
    /// The list or the tree; another one replaces it when Tree View changes.
    list: RefCell<List>,
}

/// What the tab shows its files in.
enum List {
    /// 2.2.1's plain list of paths, by row.
    Flat(Table),
    /// The folder tree, by node.
    Tree(Tree),
}

/// The folder tree: an outline over `Detail::file_tree`'s nodes.
struct Tree {
    scroll: Retained<NSScrollView>,
    outline: Retained<NSOutlineView>,
    source: Retained<RPTableSource>,
    /// What it showed at the last refresh.
    shown: RefCell<Shape>,
}

/// What decides how much of the tree a refresh redraws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Shape {
    torrent: Option<TorrentId>,
    /// The file list's identity: the engine's list again (after a priority change or Relocate)
    /// reloads the rows, keeping the open folders.
    files: usize,
    tree: bool,
    filter: String,
}

/// What a command acts on.
enum Targets {
    /// Rows of the plain list.
    Rows(Vec<usize>),
    /// Tree nodes.
    Nodes(Vec<usize>),
}

/// One file or folder of the tab, for Open, Open Containing Folder and Relocate.
#[derive(Clone, Copy)]
enum At {
    Row(usize),
    Node(usize),
}

impl At {
    /// The torrent's save path and the file's or folder's path; `None` in a remote window.
    fn path(self) -> Option<(PathBuf, PathBuf)> {
        peek(|c| match self {
            At::Row(r) => c.file_path(r),
            At::Node(n) => c.file_node_path(n),
        })
        .flatten()
    }
}

/// The Files tab's menu (`local`: the files are on this Mac, not a remote server's).
pub fn menu_items(local: bool) -> Vec<Item> {
    let c = |cmd: Command, title: &str| Item::Cmd { cmd, title: ui::text(title), key: None };
    let mut items = Vec::new();
    if local {
        items.push(c(Command::FileOpen, tr!(MenuOpenFile)));
        items.push(c(Command::FileOpenFolder, tr!(MenuOpenFolder)));
        items.push(Item::Sep);
    }
    items.push(c(Command::FileByOrder, tr!(MenuPrioritizeByOrder)));
    let last = PRIORITIES.len() - 1;
    let levels = PRIORITIES[..last]
        .iter()
        .enumerate()
        .map(|(i, &(_, prio))| c(Command::FilePriority(i as u16), priority_text(prio)))
        .collect();
    items.push(Item::Sub { title: ui::text(tr!(ColPriority)), items: levels, role: Role::Plain });
    items.push(Item::Sep);
    items.push(c(Command::FilePriority(last as u16), priority_text(PRIORITIES[last].1)));
    if local {
        items.push(Item::Sep);
        items.push(c(Command::FileRelocate, tr!(MenuRelocate)));
    }
    items.push(Item::Sep);
    // checked while the tab shows the tree (`detail::state`)
    items.push(c(Command::FilesTree, tr!(MenuFilesTree)));
    items
}

fn local() -> bool {
    peek(|c| !c.is_remote()).unwrap_or(false)
}

/// The plain list: `Detail`'s rows and cells.
fn flat_list(mtm: MainThreadMarker) -> Table {
    let mut hooks = tables::hooks(Tab::Files);
    hooks.activate = Some(Box::new(|row| open(At::Row(row as usize), false)));
    hooks.key = Some(Box::new(|k, rows| match (k, rows.first()) {
        (Key::Return, Some(&row)) => {
            open(At::Row(row as usize), false);
            true
        }
        _ => false,
    }));
    hooks.menu =
        Some(Box::new(|_rows| Some(menu::build("", &menu_items(local()), MainThreadMarker::new()?))));
    Table::new(Tab::Files, tables::specs(Tab::Files.columns()), hooks, mtm)
}

/// The node of a tree item's id.
fn node_of(item: u64) -> Option<usize> {
    item.checked_sub(1).map(|n| n as usize)
}

/// The node in outline row `row`.
fn node_at(outline: &NSOutlineView, row: isize) -> Option<usize> {
    let item = outline.itemAtRow(row)?;
    node_of(item.downcast_ref::<NSNumber>()?.as_u64())
}

/// Drops the outline's selection.
fn deselect(outline: &NSOutlineView) {
    outline.selectRowIndexes_byExtendingSelection(&table::index_set(Vec::new()), false);
}

/// Whether tree node `n` is a folder (`None`: no such node).
fn is_folder(n: usize) -> Option<bool> {
    peek(|c| c.detail().file_tree()?.node(n).map(|r| r.folder.is_some())).flatten()
}

impl Tree {
    fn new(mtm: MainThreadMarker) -> Tree {
        let mut hooks = Hooks::flat(
            || 0,
            |item, col, _dark| {
                let Some(n) = node_of(item) else { return CellValue::Empty };
                peek(|c| CellValue::Text(c.detail().file_node_cell(n, col))).unwrap_or(CellValue::Empty)
            },
        );
        hooks.children = Some(Box::new(|parent| {
            let parent = match parent {
                None => None,
                Some(p) => match node_of(p) {
                    Some(n) => Some(n),
                    None => return Vec::new(),
                },
            };
            peek(|c| {
                let t = c.detail().file_tree()?;
                Some(t.children(parent).into_iter().map(|n| n as u64 + 1).collect())
            })
            .flatten()
            .unwrap_or_default()
        }));
        hooks.type_select = Some(Box::new(|item| {
            let n = node_of(item)?;
            peek(|c| c.detail().file_tree()?.node(n).map(|r| r.name.to_owned())).flatten()
        }));
        // a file opens; a folder opens or closes (the outline's own triangle does too)
        hooks.activate = Some(Box::new(|item| {
            let Some(n) = node_of(item) else { return };
            match is_folder(n) {
                Some(true) => {
                    if let Some(t) = super::tabs() {
                        t.files.toggle_clicked_folder();
                    }
                }
                Some(false) => open(At::Node(n), false),
                None => {}
            }
        }));
        let (scroll, outline, source) =
            table::tree_table(TableKind("files-tree"), tables::specs(Tab::Files.columns()), hooks, mtm);
        // the outline has no menu hook: its menu is set once (it acts on the clicked or the
        // selected rows, `Tree::targets`)
        // SAFETY: the outline keeps its menu; the items' target is the shared RPTarget.
        unsafe { outline.setMenu(Some(&menu::build("", &menu_items(local()), mtm))) };
        let shown = RefCell::new(shape());
        Tree { scroll, outline, source, shown }
    }

    /// What the menu acts on: the selected rows' nodes, or the clicked row's when it is not one
    /// of them (a right click on another row).
    fn targets(&self) -> Vec<usize> {
        let clicked = self.outline.clickedRow();
        let selected = table::rows_of_set(&self.outline.selectedRowIndexes());
        let rows = match usize::try_from(clicked) {
            Ok(c) if !selected.contains(&c) => vec![c],
            _ => selected,
        };
        rows.into_iter().filter_map(|r| node_at(&self.outline, r as isize)).collect()
    }

    fn expand_all(&self) {
        // SAFETY: nil expands every top-level item and, with true, everything below.
        unsafe { self.outline.expandItem_expandChildren(None, true) };
    }

    /// Another torrent, its first file list or another filter: the tree again with every
    /// folder open (another torrent: at the top, nothing selected). The engine's list again
    /// (new priorities, a relocated file): the rows reload, the open folders stay open.
    /// Otherwise the rows on screen are redrawn (the progress).
    fn refresh(&self) {
        let now = shape();
        let before = self.shown.replace(now.clone());
        if before.torrent != now.torrent {
            deselect(&self.outline);
            self.source.reload_keep_selection();
            self.expand_all();
            if self.outline.numberOfRows() > 0 {
                self.outline.scrollRowToVisible(0);
            }
        } else if before.tree != now.tree || before.filter != now.filter {
            self.source.reload_keep_selection();
            self.expand_all();
        } else if before.files != now.files {
            self.source.reload_keep_selection();
        } else {
            self.source.reload_visible();
        }
    }
}

/// The tree's [`Shape`] now.
fn shape() -> Shape {
    peek(|c| {
        let d = c.detail();
        Shape {
            torrent: d.id,
            files: d.files.as_ref().map_or(0, |f| Arc::as_ptr(f).cast::<u8>() as usize),
            tree: d.file_tree().is_some(),
            filter: d.file_filter.text().to_owned(),
        }
    })
    .unwrap_or_default()
}

impl List {
    fn new(tree: bool, mtm: MainThreadMarker) -> List {
        if tree { List::Tree(Tree::new(mtm)) } else { List::Flat(flat_list(mtm)) }
    }

    fn scroll(&self) -> &NSScrollView {
        match self {
            List::Flat(t) => &t.scroll,
            List::Tree(t) => &t.scroll,
        }
    }

    fn refresh(&self) {
        match self {
            List::Flat(t) => t.refresh(),
            List::Tree(t) => t.refresh(),
        }
    }

    fn deselect(&self) {
        match self {
            List::Flat(t) => t.deselect(),
            List::Tree(t) => deselect(&t.outline),
        }
    }

    fn targets(&self) -> Targets {
        match self {
            List::Flat(t) => Targets::Rows(t.selected()),
            List::Tree(t) => Targets::Nodes(t.targets()),
        }
    }

    /// Lets go of the view (another one replaces it).
    fn detach(&self) {
        self.scroll().removeFromSuperview();
        match self {
            List::Flat(t) => table::detach(&t.view),
            List::Tree(t) => table::detach(&t.outline),
        }
    }
}

impl Files {
    pub fn new(mtm: MainThreadMarker) -> Files {
        let tree = peek(|c| c.detail().files_tree()).unwrap_or(true);
        let list = List::new(tree, mtm);

        let filter = ui::search_field(&ui::text(tr!(FilesFilterCue)), Command::FileFilter, mtm);
        // every keystroke filters, as the torrent filter does
        filter.setSendsSearchStringImmediately(true);
        layout::width(&filter, 200.0);
        let count = ui::label("", mtm);
        count.setTextColor(Some(&NSColor::secondaryLabelColor()));
        count.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
        count.setHidden(true);
        let toggle = ui::check(&ui::text(tr!(FilesTree)), tree, Command::FilesTree, mtm);
        ui::tooltip(&toggle, &ui::text(tr!(FilesTreeTip)));
        let spacer = layout::spacer(mtm);
        let bar = layout::hstack(&[&spacer, &toggle, &count, &filter], mtm);
        layout::insets(&bar, 4.0, 8.0, 4.0, 6.0);

        let view = NSView::new(mtm);
        view.addSubview(&bar);
        layout::activate(&[
            bar.topAnchor().constraintEqualToAnchor(&view.topAnchor()),
            bar.leadingAnchor().constraintEqualToAnchor(&view.leadingAnchor()),
            bar.trailingAnchor().constraintEqualToAnchor(&view.trailingAnchor()),
        ]);
        let files = Files { view, filter, count, toggle, bar, list: RefCell::new(list) };
        files.place();
        files
    }

    /// Puts the list or tree under the filter's row.
    fn place(&self) {
        let list = self.list.borrow();
        let scroll = list.scroll();
        self.view.addSubview(scroll);
        layout::activate(&[
            scroll.topAnchor().constraintEqualToAnchor(&self.bar.bottomAnchor()),
            scroll.leadingAnchor().constraintEqualToAnchor(&self.view.leadingAnchor()),
            scroll.trailingAnchor().constraintEqualToAnchor(&self.view.trailingAnchor()),
            scroll.bottomAnchor().constraintEqualToAnchor(&self.view.bottomAnchor()),
        ]);
    }

    /// The list and the count beside the filter.
    pub fn refresh(&self) {
        let count = peek(|c| c.detail().file_filter_count()).unwrap_or_default();
        if self.count.stringValue().to_string() != count {
            self.count.setStringValue(&NSString::from_str(&count));
        }
        self.count.setHidden(count.is_empty());
        self.list.borrow().refresh();
    }

    /// The filter field's text changed: the rows it lets through (the selection, by row, starts
    /// over).
    pub fn filter_changed(&self) {
        let text = self.filter.stringValue().to_string();
        if with_core(|c| c.set_file_filter(&text)) == Some(true) {
            self.list.borrow().deselect();
            apply(with_core(|c| c.events()));
        }
    }

    /// Tree View (the check box or the menu): the folder tree or the plain list, in a new view
    /// (the tree with every folder open); the choice is kept.
    fn toggle_tree(&self) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let on = !peek(|c| c.detail().files_tree()).unwrap_or(true);
        let changed = with_core(|c| {
            c.prefs_mut().set_bool(FILES_TREE_KEY, on);
            c.set_files_tree(on)
        });
        ui::set_checked(&self.toggle, on);
        if changed != Some(true) {
            return;
        }
        let window = self.view.window();
        let had_focus = window.as_deref().is_some_and(|w| super::focus_in(w, self.list.borrow().scroll()));
        let old = self.list.replace(List::new(on, mtm));
        old.detach();
        drop(old);
        self.place();
        if let (true, Some(w)) = (had_focus, &window) {
            let list = self.list.borrow();
            let view: &NSView = match &*list {
                List::Flat(t) => &t.view,
                List::Tree(t) => &t.outline,
            };
            w.makeFirstResponder(Some(view));
        }
        apply(with_core(|c| c.events()));
    }

    /// A double click on a folder of the tree: it opens, or closes when open.
    fn toggle_clicked_folder(&self) {
        let list = self.list.borrow();
        let List::Tree(t) = &*list else { return };
        let Some(item) = t.outline.itemAtRow(t.outline.clickedRow()) else { return };
        let item: &AnyObject = &item;
        // SAFETY: the item is one of the outline's own (from `itemAtRow:`).
        unsafe {
            if t.outline.isItemExpanded(Some(item)) {
                t.outline.collapseItem(Some(item));
            } else {
                t.outline.expandItem(Some(item));
            }
        }
    }

    /// Whether the command applies now (the menu's validation).
    pub fn state(&self, cmd: Command) -> bool {
        let local = local();
        let (n, one_file) = match self.list.borrow().targets() {
            Targets::Rows(rows) => (rows.len(), rows.len() == 1),
            Targets::Nodes(nodes) => (nodes.len(), nodes.len() == 1 && is_folder(nodes[0]) == Some(false)),
        };
        match cmd {
            Command::FileFilter | Command::FilesTree => true,
            Command::FileOpen | Command::FileOpenFolder => local && n > 0,
            // Relocate takes one file, as in µTorrent
            Command::FileRelocate => local && one_file,
            _ => n > 0,
        }
    }

    /// Runs one of the tab's commands.
    pub fn run(&self, cmd: Command) {
        match cmd {
            Command::FileFilter => return self.filter_changed(),
            Command::FilesTree => return self.toggle_tree(),
            _ => {}
        }
        // no borrow of the list while the command runs: it redraws the list
        let targets = self.list.borrow().targets();
        let first = match &targets {
            Targets::Rows(rows) => rows.first().map(|&r| At::Row(r)),
            Targets::Nodes(nodes) => nodes.first().map(|&n| At::Node(n)),
        };
        let prio = match cmd {
            Command::FileOpen => return first.into_iter().for_each(|at| open(at, false)),
            Command::FileOpenFolder => return first.into_iter().for_each(|at| open(at, true)),
            Command::FileRelocate => return first.into_iter().for_each(relocate),
            Command::FileByOrder => None,
            Command::FilePriority(i) => match PRIORITIES.get(i as usize) {
                Some(&(_, prio)) => Some(prio),
                None => return,
            },
            _ => return,
        };
        apply(with_core(|c| {
            match &targets {
                Targets::Rows(rows) => c.set_file_priority(rows, prio),
                Targets::Nodes(nodes) => {
                    let files =
                        c.detail().file_tree().map(|t| t.files_under_nodes(nodes)).unwrap_or_default();
                    c.set_file_priority_files(&files, prio);
                }
            }
            c.events()
        }));
    }
}

fn file_url(p: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()))
}

/// Open (a double click, Return, the menu) and Open Containing Folder for a file (or, in the
/// tree, a folder). Open hands a file to its default application and shows a folder in
/// Finder; what is not on disk yet does nothing. Open Containing Folder shows it selected in
/// Finder once it exists, else the nearest folder inside the save path that does.
fn open(at: At, folder: bool) {
    let Some((save, path)) = at.path() else { return };
    let ws = NSWorkspace::sharedWorkspace();
    if folder && path.exists() {
        ws.activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[file_url(&path)]));
    } else if folder {
        let shown = path.ancestors().skip(1).take_while(|p| p.starts_with(&save)).find(|p| p.is_dir());
        ws.openURL(&file_url(shown.unwrap_or(&save)));
    } else if path.exists() {
        ws.openURL(&file_url(&path));
    }
}

/// Relocate for a file: it moves where the user picks, or, when it is not on disk, the torrent
/// looks for it there (a Force Re-check counts what is there).
fn relocate(at: At) {
    let Some((_, path)) = at.path() else { return };
    let Some(mtm) = MainThreadMarker::new() else { return };
    let window = crate::app::ui().map(|u| u.window.clone());
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let start: Option<PathBuf> = path.parent().filter(|d| d.is_dir()).map(Path::to_path_buf);
    // the save panel asks before "replacing" a file; the engine never overwrites one
    ui::save_panel(
        window.as_deref(),
        &name,
        start,
        move |to| {
            let Some(to) = to.filter(|to| *to != path) else { return };
            apply(with_core(|c| {
                match at {
                    At::Row(r) => c.relocate_file(r, &to),
                    At::Node(n) => c.relocate_file_node(n, &to),
                }
                c.events()
            }));
        },
        mtm,
    );
}
