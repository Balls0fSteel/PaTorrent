//! The Files tab: the torrent's files (`FILE_COLUMNS`) under the D50 filter field at the tab's
//! top right, with "N of M files" beside it while it hides some (the field's own button clears
//! it). Its menu, as µTorrent 2.2.1's (but for Scan for Viruses): Open and Open Containing
//! Folder when the files are on this Mac; Prioritize by File Order and Priority ▸; Don't
//! Download; Relocate (one file, on this Mac). A double click or Return opens a file; ⌘A selects
//! the files the filter shows (the table's own Select All); ⌘F in the tab goes to the field.

use std::path::{Path, PathBuf};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFont, NSSearchField, NSTextField, NSView, NSWorkspace};
use objc2_foundation::{NSArray, NSString, NSURL};
use rookparcel_frontend::detail::{PRIORITIES, Tab, priority_text};
use rookparcel_frontend::tr;

use super::tables::{self, Table};
use crate::app::{apply, peek, with_core};
use crate::commands::Command;
use crate::menu::{self, Item, Role};
use crate::table::Key;
use crate::ui::{self, layout};

/// The tab's views.
pub struct Files {
    pub view: Retained<NSView>,
    pub filter: Retained<NSSearchField>,
    count: Retained<NSTextField>,
    pub list: Table,
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
    items
}

impl Files {
    pub fn new(mtm: MainThreadMarker) -> Files {
        let mut hooks = tables::hooks(Tab::Files);
        hooks.activate = Some(Box::new(|row| open(row as usize, false)));
        hooks.key = Some(Box::new(|k, rows| match (k, rows.first()) {
            (Key::Return, Some(&row)) => {
                open(row as usize, false);
                true
            }
            _ => false,
        }));
        hooks.menu = Some(Box::new(|_rows| {
            let local = peek(|c| !c.is_remote()).unwrap_or(false);
            Some(menu::build("", &menu_items(local), MainThreadMarker::new()?))
        }));
        let list = Table::new(Tab::Files, tables::specs(Tab::Files.columns()), hooks, mtm);

        let filter = ui::search_field(&ui::text(tr!(FilesFilterCue)), Command::FileFilter, mtm);
        // every keystroke filters, as the torrent filter does
        filter.setSendsSearchStringImmediately(true);
        layout::width(&filter, 200.0);
        let count = ui::label("", mtm);
        count.setTextColor(Some(&NSColor::secondaryLabelColor()));
        count.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
        count.setHidden(true);
        let spacer = layout::spacer(mtm);
        let bar = layout::hstack(&[&spacer, &count, &filter], mtm);
        layout::insets(&bar, 4.0, 8.0, 4.0, 6.0);

        let view = NSView::new(mtm);
        view.addSubview(&bar);
        view.addSubview(&list.scroll);
        layout::activate(&[
            bar.topAnchor().constraintEqualToAnchor(&view.topAnchor()),
            bar.leadingAnchor().constraintEqualToAnchor(&view.leadingAnchor()),
            bar.trailingAnchor().constraintEqualToAnchor(&view.trailingAnchor()),
            list.scroll.topAnchor().constraintEqualToAnchor(&bar.bottomAnchor()),
            list.scroll.leadingAnchor().constraintEqualToAnchor(&view.leadingAnchor()),
            list.scroll.trailingAnchor().constraintEqualToAnchor(&view.trailingAnchor()),
            list.scroll.bottomAnchor().constraintEqualToAnchor(&view.bottomAnchor()),
        ]);
        Files { view, filter, count, list }
    }

    /// The list and the count beside the filter.
    pub fn refresh(&self) {
        let count = peek(|c| c.detail().file_filter_count()).unwrap_or_default();
        if self.count.stringValue().to_string() != count {
            self.count.setStringValue(&NSString::from_str(&count));
        }
        self.count.setHidden(count.is_empty());
        self.list.refresh();
    }

    /// The filter field's text changed: the rows it lets through (the selection, by row, starts
    /// over).
    pub fn filter_changed(&self) {
        let text = self.filter.stringValue().to_string();
        if with_core(|c| c.set_file_filter(&text)) == Some(true) {
            self.list.deselect();
            apply(with_core(|c| c.events()));
        }
    }

    /// Whether the command applies now (the menu's validation).
    pub fn state(&self, cmd: Command) -> bool {
        let rows = self.list.selected().len();
        let local = peek(|c| !c.is_remote()).unwrap_or(false);
        match cmd {
            Command::FileFilter => true,
            Command::FileOpen | Command::FileOpenFolder => local && rows > 0,
            // Relocate takes one file, as in µTorrent
            Command::FileRelocate => local && rows == 1,
            _ => rows > 0,
        }
    }

    /// Runs one of the tab's commands.
    pub fn run(&self, cmd: Command) {
        let rows = self.list.selected();
        let first = rows.first().copied();
        match cmd {
            Command::FileFilter => self.filter_changed(),
            Command::FileOpen => first.into_iter().for_each(|r| open(r, false)),
            Command::FileOpenFolder => first.into_iter().for_each(|r| open(r, true)),
            Command::FileRelocate => first.into_iter().for_each(relocate),
            Command::FileByOrder => apply(with_core(|c| {
                c.set_file_priority(&rows, None);
                c.events()
            })),
            Command::FilePriority(i) => {
                let Some(&(_, prio)) = PRIORITIES.get(i as usize) else { return };
                apply(with_core(|c| {
                    c.set_file_priority(&rows, Some(prio));
                    c.events()
                }));
            }
            _ => {}
        }
    }
}

fn file_url(p: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()))
}

/// Open (a double click, Return, the menu) and Open Containing Folder for row `row`. Open hands
/// the file to its default application; a file not on disk yet does nothing. The folder shows
/// the file selected in Finder once it exists, else the nearest folder inside the save path
/// that does.
pub fn open(row: usize, folder: bool) {
    let Some((save, path)) = peek(|c| c.file_path(row)).flatten() else { return };
    let ws = NSWorkspace::sharedWorkspace();
    if folder && path.exists() {
        ws.activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[file_url(&path)]));
    } else if folder {
        let shown = path.ancestors().skip(1).take_while(|p| p.starts_with(&save)).find(|p| p.is_dir());
        ws.openURL(&file_url(shown.unwrap_or(&save)));
    } else if path.is_file() {
        ws.openURL(&file_url(&path));
    }
}

/// Relocate for row `row`: the file moves where the user picks, or, when it is not on disk,
/// the torrent looks for it there (a Force Re-check counts what is there).
pub fn relocate(row: usize) {
    let Some((_, path)) = peek(|c| c.file_path(row)).flatten() else { return };
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
                c.relocate_file(row, &to);
                c.events()
            }));
        },
        mtm,
    );
}
