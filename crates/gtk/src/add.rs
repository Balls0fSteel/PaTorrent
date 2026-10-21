//! The Add New Torrent dialog (µTorrent 2.2.1), as the Win32 one: Save In with the recent
//! folders, Name, the options and Label on the left; the torrent's contents with a file tree of
//! check boxes and priorities on the right.
//!
//! The file tree is a `GtkColumnView` over a [`CountModel`] whose rows are the visible rows of
//! the frontend's [`FileTree`], so torrents with tens of thousands of files open at once.
//! Opening or closing a folder splices rows in or out under it; the cells read their row from
//! the list item's position whenever they are drawn. Folders open and close with their arrow,
//! a double click, Enter, Right / Left or + / -; Space toggles the selected rows' check boxes;
//! a right click opens the priorities.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_core::metainfo::TorrentInfo;
use patorrent_frontend::add::{self, AddChoice, Check, FileTree, Setup};
use patorrent_frontend::detail::{PRIORITIES, priority_text};
use patorrent_frontend::lang::{self, gtk_mnemonic};
use patorrent_frontend::{fmt, tr};

use crate::table::CountModel;

/// Called once with the choice, or `None` for Cancel (and a closed window).
type Done = Box<dyn FnOnce(Option<AddChoice>)>;

/// The name column's parts.
struct NameCell {
    expander: gtk::Button,
    check: gtk::CheckButton,
    icon: gtk::Image,
    label: gtk::Label,
}

enum Widgets {
    Name(NameCell),
    Size(gtk::Label),
    Priority(gtk::Label),
}

/// A cell on screen.
struct Bound {
    item: glib::WeakRef<gtk::ListItem>,
    widgets: Rc<Widgets>,
}

/// What the dialog's handlers share. It holds no window or view, so the widgets' closures can
/// keep it without a cycle; the window's close handler keeps it alive.
struct Dialog {
    tree: RefCell<FileTree>,
    model: CountModel,
    selection: gtk::MultiSelection,
    bound: RefCell<Vec<Bound>>,
    /// Set while the cells are drawn, so setting a check box is not taken for a click.
    drawing: Cell<bool>,
    save: gtk::Entry,
    size: gtk::Label,
    /// The theme's icon per lower-case extension.
    icons: RefCell<HashMap<String, gio::Icon>>,
    done: RefCell<Option<Done>>,
}

/// Shows the dialog for `ti` over `parent`; `done` gets the choice (`None` on Cancel).
pub fn show(
    parent: &gtk::ApplicationWindow,
    ti: &TorrentInfo,
    setup: Setup,
    done: impl FnOnce(Option<AddChoice>) + 'static,
) {
    let window = gtk::Window::builder()
        .title(tr!(AddTitle))
        .transient_for(parent)
        .modal(true)
        .default_width(780)
        .default_height(500)
        .build();

    // Save In: the folder, the remembered ones and a folder picker
    let (save_row, save) = entry_with_choices(&setup.save_dirs);
    save.set_text(setup.save_dirs.first().map_or("", String::as_str));
    save.set_hexpand(true);
    let browse = gtk::Button::with_label("...");
    // a folder on this computer means nothing to a remote server: the path is typed
    browse.set_sensitive(!crate::remote::is_remote());
    save_row.append(&browse);
    let name = gtk::Entry::new();
    name.set_text(ti.name());
    name.set_activates_default(true);
    save.set_activates_default(true);

    let skip = gtk::CheckButton::with_mnemonic(&gtk_mnemonic(tr!(AddSkipCheck)));
    skip.set_sensitive(!setup.in_session);
    let start = gtk::CheckButton::with_mnemonic(&gtk_mnemonic(tr!(AddStart)));
    start.set_active(setup.start);
    let top = gtk::CheckButton::with_mnemonic(&gtk_mnemonic(tr!(AddTopOfQueue)));
    let (label_row, label) = entry_with_choices(&setup.labels);
    label.set_hexpand(true);
    label.set_activates_default(true);
    let label_caption = gtk::Label::with_mnemonic(&gtk_mnemonic(tr!(AddLabel)));
    label_caption.set_mnemonic_widget(Some(&label));
    let label_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    label_line.append(&label_caption);
    label_line.append(&label_row);
    let options = gtk::Box::new(gtk::Orientation::Vertical, 4);
    for w in [skip.upcast_ref::<gtk::Widget>(), start.upcast_ref(), top.upcast_ref()] {
        options.append(w);
    }
    label_line.set_margin_top(8);
    options.append(&label_line);

    let left = gtk::Box::new(gtk::Orientation::Vertical, 12);
    left.set_size_request(280, -1);
    left.append(&framed(tr!(AddSaveIn), &save_row, Some(&save)));
    left.append(&framed(tr!(AddName), &name, Some(&name)));
    left.append(&framed(tr!(AddOptions), &options, None));

    // Torrent Contents
    let info = gtk::Grid::new();
    info.set_column_spacing(12);
    info.set_row_spacing(2);
    let size = value_label("");
    let date = ti.creation_date().map(fmt::date_time).unwrap_or_default();
    for (row, (caption, value)) in [
        (tr!(AddTorrentName), value_label(ti.name())),
        (tr!(AddComment), value_label(ti.comment())),
        (tr!(AddSize), size.clone()),
        (tr!(AddDate), value_label(&date)),
    ]
    .into_iter()
    .enumerate()
    {
        let c = gtk::Label::new(Some(caption));
        c.set_xalign(0.0);
        info.attach(&c, 0, row as i32, 1, 1);
        info.attach(&value, 1, row as i32, 1, 1);
    }

    let model = CountModel::default();
    let selection = gtk::MultiSelection::new(Some(model.clone()));
    let view = gtk::ColumnView::new(Some(selection.clone()));
    view.set_reorderable(false);
    view.set_show_column_separators(true);
    view.add_css_class("data-table");
    let d = Rc::new(Dialog {
        tree: RefCell::new(FileTree::from_torrent(ti)),
        model,
        selection,
        bound: RefCell::default(),
        drawing: Cell::new(false),
        save: save.clone(),
        size,
        icons: RefCell::default(),
        done: RefCell::new(Some(Box::new(done))),
    });
    let menu = files_menu();
    for (column, title, width) in [(0, tr!(ColName), 300), (1, tr!(ColSize), 90), (2, tr!(ColPriority), 90)] {
        let col = gtk::ColumnViewColumn::new(Some(title), Some(factory(&d, column, &view, &menu)));
        col.set_resizable(true);
        col.set_fixed_width(width);
        col.set_expand(column == 0);
        view.append_column(&col);
    }
    d.model.set_len(d.tree.borrow().len());
    let files =
        gtk::ScrolledWindow::builder().child(&view).hexpand(true).vexpand(true).has_frame(true).build();
    let all = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(MenuSelectAll)));
    let none = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(MenuSelectNone)));
    let select = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    select.append(&all);
    select.append(&none);
    let contents = gtk::Box::new(gtk::Orientation::Vertical, 8);
    contents.append(&info);
    contents.append(&files);
    contents.append(&select);
    let right = framed(tr!(AddContents), &contents, None);
    right.set_hexpand(true);

    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    columns.set_vexpand(true);
    columns.append(&left);
    columns.append(&right);

    let ok = gtk::Button::with_label(tr!(BtnOk));
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label(tr!(BtnCancel));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&ok);
    buttons.append(&cancel);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&columns);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&ok));
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);

    // the file list's menu: priorities, check and uncheck for the selected rows
    let group = gio::SimpleActionGroup::new();
    for (i, &(_, prio)) in PRIORITIES.iter().enumerate() {
        let a = gio::SimpleAction::new(&format!("prio-{i}"), None);
        let w = Rc::downgrade(&d);
        a.connect_activate(move |_, _| {
            if let Some(d) = w.upgrade() {
                let rows = d.selected();
                d.tree.borrow_mut().set_priority(&rows, prio);
                d.changed();
            }
        });
        group.add_action(&a);
    }
    for (name, on) in [("check", true), ("uncheck", false)] {
        let a = gio::SimpleAction::new(name, None);
        let w = Rc::downgrade(&d);
        a.connect_activate(move |_, _| {
            if let Some(d) = w.upgrade() {
                let rows = d.selected();
                d.tree.borrow_mut().set_checks(&rows, on);
                d.changed();
            }
        });
        group.add_action(&a);
    }
    window.insert_action_group("add", Some(&group));

    // folders open and close; Space checks
    view.connect_activate({
        let w = Rc::downgrade(&d);
        move |_, position| {
            if let Some(d) = w.upgrade() {
                d.expand(position as usize, None);
            }
        }
    });
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    key.connect_key_pressed({
        let w = Rc::downgrade(&d);
        move |_, key, _, _| {
            use gtk::gdk::Key;
            let Some(d) = w.upgrade() else { return glib::Propagation::Proceed };
            let rows = d.selected();
            let Some(&row) = rows.first() else { return glib::Propagation::Proceed };
            if key == Key::space {
                d.tree.borrow_mut().toggle_checks(&rows);
                d.changed();
            } else if matches!(key, Key::Right | Key::plus | Key::KP_Add) {
                d.expand(row, Some(true));
            } else if matches!(key, Key::Left | Key::minus | Key::KP_Subtract) {
                if !d.expand(row, Some(false)) && key == Key::Left {
                    // a file or a closed folder: to the folder holding it
                    let parent = d.tree.borrow().parent_row(row);
                    if let Some(p) = parent {
                        d.selection.select_item(p as u32, true);
                    }
                }
            } else {
                return glib::Propagation::Proceed;
            }
            glib::Propagation::Stop
        }
    });
    view.add_controller(key);

    all.connect_clicked({
        let w = Rc::downgrade(&d);
        move |_| {
            if let Some(d) = w.upgrade() {
                d.tree.borrow_mut().set_all(true);
                d.changed();
            }
        }
    });
    none.connect_clicked({
        let w = Rc::downgrade(&d);
        move |_| {
            if let Some(d) = w.upgrade() {
                d.tree.borrow_mut().set_all(false);
                d.changed();
            }
        }
    });
    save.connect_changed({
        let w = Rc::downgrade(&d);
        move |_| {
            if let Some(d) = w.upgrade() {
                d.update_size();
            }
        }
    });
    browse.connect_clicked({
        let (window, save) = (window.downgrade(), save.downgrade());
        move |_| {
            let (Some(window), Some(save)) = (window.upgrade(), save.upgrade()) else { return };
            let picker = gtk::FileDialog::builder().title(tr!(AddPickFolder)).modal(true).build();
            let current = save.text();
            if Path::new(current.trim()).is_dir() {
                picker.set_initial_folder(Some(&gio::File::for_path(current.trim())));
            }
            picker.select_folder(Some(&window), None::<&gio::Cancellable>, move |r| {
                if let Some(path) = r.ok().and_then(|f| f.path()) {
                    save.set_text(&path.to_string_lossy());
                }
            });
        }
    });
    cancel.connect_clicked({
        let window = window.downgrade();
        move |_| {
            if let Some(w) = window.upgrade() {
                w.close();
            }
        }
    });
    ok.connect_clicked({
        let w = Rc::downgrade(&d);
        let window = window.downgrade();
        let (name, skip, start, top, label) =
            (name.downgrade(), skip.downgrade(), start.downgrade(), top.downgrade(), label.downgrade());
        let (torrent_name, in_session) = (ti.name().to_owned(), setup.in_session);
        move |_| {
            let (Some(d), Some(window)) = (w.upgrade(), window.upgrade()) else { return };
            let (Some(name), Some(skip), Some(start), Some(top), Some(label)) =
                (name.upgrade(), skip.upgrade(), start.upgrade(), top.upgrade(), label.upgrade())
            else {
                return;
            };
            let save_path = match add::save_folder(&d.save.text()) {
                Ok(dir) => dir,
                Err(e) => {
                    gtk::AlertDialog::builder().message(e).modal(true).build().show(Some(&window));
                    d.save.grab_focus();
                    return;
                }
            };
            let new_name = name.text().trim().to_owned();
            let choice = AddChoice {
                save_path,
                name: (!new_name.is_empty() && new_name != torrent_name).then_some(new_name),
                file_priorities: d.tree.borrow().file_priorities(),
                start: start.is_active(),
                skip_hash_check: !in_session && skip.is_active(),
                top_of_queue: top.is_active(),
                label: label.text().trim().to_owned(),
            };
            let done = d.done.borrow_mut().take();
            window.close();
            if let Some(done) = done {
                done(Some(choice));
            }
        }
    });
    // Cancel, Escape and the title bar's close all end here; the handler keeps the shared
    // state until the window goes
    window.connect_close_request({
        let d = d.clone();
        move |_| {
            let done = d.done.borrow_mut().take();
            if let Some(done) = done {
                done(None);
            }
            glib::Propagation::Proceed
        }
    });

    d.update_size();
    window.present();
    ok.grab_focus();
}

impl Dialog {
    /// The selected rows, ascending.
    fn selected(&self) -> Vec<usize> {
        let set = self.selection.selection();
        (0..set.size() as u32).map(|i| set.nth(i) as usize).collect()
    }

    /// After check or priority changes: the cells and the size.
    fn changed(&self) {
        self.redraw();
        self.update_size();
    }

    /// Opens (`Some(true)`), closes (`Some(false)`) or toggles the folder in `row`; the rows
    /// below it come and go and the folder's row stays selected. False for files and folders
    /// already that way.
    fn expand(&self, row: usize, to: Option<bool>) -> bool {
        let (old, changed, new) = {
            let mut tree = self.tree.borrow_mut();
            let old = tree.len();
            let changed = tree.set_expanded(row, to);
            (old, changed, tree.len())
        };
        if !changed {
            return false;
        }
        if new > old {
            self.model.splice(row + 1, 0, new - old);
        } else {
            self.model.splice(row + 1, old - new, 0);
        }
        self.selection.select_item(row as u32, true);
        self.redraw();
        true
    }

    /// Draws every cell on screen from the tree.
    fn redraw(&self) {
        let cells: Vec<(u32, Rc<Widgets>)> = self
            .bound
            .borrow()
            .iter()
            .filter_map(|b| b.item.upgrade().map(|i| (i.position(), b.widgets.clone())))
            .collect();
        for (position, widgets) in cells {
            self.draw(position, &widgets);
        }
    }

    fn draw(&self, position: u32, widgets: &Widgets) {
        if position == gtk::INVALID_LIST_POSITION {
            return;
        }
        let tree = self.tree.borrow();
        let Some(row) = tree.row(position as usize) else { return };
        self.drawing.set(true);
        match widgets {
            Widgets::Name(c) => {
                c.expander.set_margin_start(i32::from(row.depth) * 16);
                c.expander.set_opacity(if row.folder.is_some() { 1.0 } else { 0.0 });
                c.expander.set_can_target(row.folder.is_some());
                c.expander.set_icon_name(match row.folder {
                    Some(true) => "pan-down-symbolic",
                    _ if lang::right_to_left() => "pan-start-symbolic",
                    _ => "pan-end-symbolic",
                });
                c.check.set_inconsistent(row.check == Check::Mixed);
                c.check.set_active(row.check == Check::On);
                c.icon.set_from_gicon(&self.icon(row.name, row.folder));
                set_text(&c.label, row.name);
            }
            Widgets::Size(l) => set_text(l, &fmt::size(row.size)),
            Widgets::Priority(l) => set_text(l, row.priority.map_or(tr!(AddMixed), priority_text)),
        }
        self.drawing.set(false);
    }

    /// The theme's icon for a folder or a file name.
    fn icon(&self, name: &str, folder: Option<bool>) -> gio::Icon {
        if let Some(open) = folder {
            return gio::ThemedIcon::new(if open { "folder-open" } else { "folder" }).upcast();
        }
        let ext = name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_lowercase());
        self.icons
            .borrow_mut()
            .entry(ext)
            .or_insert_with_key(|ext| {
                let (kind, _) = gio::content_type_guess(Some(format!("file.{ext}")), None);
                gio::content_type_get_icon(&kind)
            })
            .clone()
    }

    /// "Size: 1.20 GB of 3.40 GB selected (disk space: 120 GB free)".
    fn update_size(&self) {
        let (sel, all) = self.tree.borrow().sizes();
        let mut text = add::size_text(sel, all);
        if let Some(free) = free_space(&self.save.text()) {
            text.push(' ');
            text.push_str(&tr!(AddDiskSpace, free = fmt::size(free)));
        }
        set_text(&self.size, &text);
    }

    /// A click on a row's check box.
    fn toggle(&self, row: usize) {
        if self.drawing.get() {
            return;
        }
        self.tree.borrow_mut().toggle_checks(&[row]);
        self.changed();
    }
}

/// The cells of one column; `menu` opens on a right click.
fn factory(
    d: &Rc<Dialog>,
    column: usize,
    view: &gtk::ColumnView,
    menu: &gio::Menu,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let (w, view, menu) = (Rc::downgrade(d), view.downgrade(), menu.clone());
    factory.connect_setup(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else { return };
        let (root, widgets): (gtk::Widget, Widgets) = match column {
            0 => {
                let expander = gtk::Button::from_icon_name("pan-end-symbolic");
                expander.add_css_class("flat");
                expander.set_focus_on_click(false);
                expander.set_valign(gtk::Align::Center);
                let check = gtk::CheckButton::new();
                check.set_focus_on_click(false);
                let icon = gtk::Image::new();
                let label = gtk::Label::new(None);
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_single_line_mode(true);
                label.set_hexpand(true);
                let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                for part in [
                    expander.upcast_ref::<gtk::Widget>(),
                    check.upcast_ref(),
                    icon.upcast_ref(),
                    label.upcast_ref(),
                ] {
                    b.append(part);
                }
                let item_ref = item.downgrade();
                expander.connect_clicked({
                    let (w, item_ref) = (w.clone(), item_ref.clone());
                    move |_| {
                        if let (Some(d), Some(item)) = (w.upgrade(), item_ref.upgrade()) {
                            d.expand(item.position() as usize, None);
                        }
                    }
                });
                check.connect_toggled({
                    let w = w.clone();
                    move |_| {
                        if let (Some(d), Some(item)) = (w.upgrade(), item_ref.upgrade()) {
                            d.toggle(item.position() as usize);
                        }
                    }
                });
                (b.upcast(), Widgets::Name(NameCell { expander, check, icon, label }))
            }
            _ => {
                let label = gtk::Label::new(None);
                label.set_xalign(if column == 1 { 1.0 } else { 0.0 });
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_single_line_mode(true);
                (
                    label.clone().upcast(),
                    if column == 1 { Widgets::Size(label) } else { Widgets::Priority(label) },
                )
            }
        };
        // a right click selects the row first (unless it is part of the selection)
        let click = gtk::GestureClick::new();
        click.set_button(3);
        let (dialog, item_ref, view, menu) = (w.clone(), item.downgrade(), view.clone(), menu.clone());
        click.connect_pressed(move |g, _, x, y| {
            let (Some(d), Some(item), Some(view)) = (dialog.upgrade(), item_ref.upgrade(), view.upgrade())
            else {
                return;
            };
            let pos = item.position();
            if pos != gtk::INVALID_LIST_POSITION && !d.selection.is_selected(pos) {
                d.selection.select_item(pos, true);
            }
            if let Some(widget) = g.widget() {
                crate::app::popup_menu(&menu, view.upcast_ref(), &widget, x, y);
            }
        });
        root.add_controller(click);
        item.set_child(Some(&root));
        if let Some(d) = w.upgrade() {
            d.bound.borrow_mut().push(Bound { item: item.downgrade(), widgets: Rc::new(widgets) });
        }
    });
    let w = Rc::downgrade(d);
    factory.connect_bind(move |_, obj| {
        let (Some(d), Some(item)) = (w.upgrade(), obj.downcast_ref::<gtk::ListItem>()) else { return };
        let widgets = d
            .bound
            .borrow()
            .iter()
            .find(|b| b.item.upgrade().as_ref() == Some(item))
            .map(|b| b.widgets.clone());
        if let Some(widgets) = widgets {
            d.draw(item.position(), &widgets);
        }
    });
    let w = Rc::downgrade(d);
    factory.connect_teardown(move |_, obj| {
        let (Some(d), Some(item)) = (w.upgrade(), obj.downcast_ref::<gtk::ListItem>()) else { return };
        d.bound.borrow_mut().retain(|b| b.item.upgrade().is_some_and(|i| &i != item));
    });
    factory
}

/// The file list's right-click menu.
fn files_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let prios = gio::Menu::new();
    for (i, (text, _)) in PRIORITIES.iter().enumerate() {
        prios.append(Some(&gtk_mnemonic(lang::text(*text))), Some(&format!("add.prio-{i}")));
    }
    menu.append_section(None, &prios);
    let checks = gio::Menu::new();
    checks.append(Some(tr!(MenuCheck)), Some("add.check"));
    checks.append(Some(tr!(MenuUncheck)), Some("add.uncheck"));
    menu.append_section(None, &checks);
    menu
}

/// A group box: `title` (its `&` mnemonic focusing `target`) over `child`.
fn framed(title: &str, child: &impl IsA<gtk::Widget>, target: Option<&gtk::Entry>) -> gtk::Frame {
    let caption = gtk::Label::with_mnemonic(&gtk_mnemonic(title));
    if let Some(t) = target {
        caption.set_mnemonic_widget(Some(t));
    }
    let frame = gtk::Frame::new(None);
    frame.set_label_widget(Some(&caption));
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.set_margin_top(6);
    inner.set_margin_bottom(8);
    inner.set_margin_start(8);
    inner.set_margin_end(8);
    inner.append(child);
    frame.set_child(Some(&inner));
    frame
}

/// An entry with a drop-down of `choices` beside it (none when there are none).
pub(crate) fn entry_with_choices(choices: &[String]) -> (gtk::Box, gtk::Entry) {
    let entry = gtk::Entry::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("linked");
    row.append(&entry);
    if choices.is_empty() {
        return (row, entry);
    }
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let popover = gtk::Popover::new();
    popover.set_child(Some(&list));
    for c in choices {
        let b = gtk::Button::with_label(c);
        b.add_css_class("flat");
        if let Some(l) = b.child().and_downcast::<gtk::Label>() {
            l.set_xalign(0.0);
        }
        let (entry, popover, c) = (entry.downgrade(), popover.downgrade(), c.clone());
        b.connect_clicked(move |_| {
            if let Some(e) = entry.upgrade() {
                e.set_text(&c);
            }
            if let Some(p) = popover.upgrade() {
                p.popdown();
            }
        });
        list.append(&b);
    }
    let more = gtk::MenuButton::new();
    more.set_popover(Some(&popover));
    row.append(&more);
    (row, entry)
}

/// A value of the Torrent Contents box: selectable, ellipsized.
fn value_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    l.set_selectable(true);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_single_line_mode(true);
    l
}

fn set_text(l: &gtk::Label, text: &str) {
    if l.text() != text {
        l.set_text(text);
    }
}

/// Free bytes on the file system of `dir` (or of its nearest existing parent).
fn free_space(dir: &str) -> Option<u64> {
    let mut p = Path::new(dir.trim());
    loop {
        if p.as_os_str().is_empty() {
            return None;
        }
        if p.is_dir() {
            let info = gio::File::for_path(p)
                .query_filesystem_info(gio::FILE_ATTRIBUTE_FILESYSTEM_FREE, None::<&gio::Cancellable>)
                .ok()?;
            return Some(info.attribute_uint64(gio::FILE_ATTRIBUTE_FILESYSTEM_FREE));
        }
        p = p.parent()?;
    }
}
