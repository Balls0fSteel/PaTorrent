//! The detail tabs' tables (Trackers, Peers, Files, Logger and the Pieces list): a
//! `GtkColumnView` over [`CountModel`], a `GListModel` that is only a row count. Rows are
//! indices into the frontend's [`Detail`](rookparcel_frontend::detail::Detail); the cells
//! on screen are remembered and redrawn from it, as the torrent list does.
//!
//! A count that grows adds rows at the end and one that shrinks drops them there, so the
//! Logger keeps its scroll position while lines arrive.
//!
//! A column can have a state icon before its text ([`Table::with_icon`]), drawn as the torrent
//! list's Name column draws its own: the Trackers tab's status icon.
//!
//! The first column can be a folder tree ([`Table::with_tree`], the Files tab, D62): indents,
//! expander arrows and folder or file icons, as the Add dialog draws its own. A folder that
//! opens or closes splices rows in or out under it ([`Table::spliced`]); the cells read their
//! row from the list item's position, so the rows that move need no new binding.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use rookparcel_frontend::detail::ColumnDef;
use rookparcel_frontend::icons::StateIcon;
use rookparcel_frontend::lang;

use crate::canvas::Canvas;

mod imp {
    use std::cell::Cell;

    use gtk::subclass::prelude::*;
    use gtk::{gio, glib, prelude::*};

    #[derive(Default)]
    pub struct CountModel {
        pub len: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CountModel {
        const NAME: &'static str = "RookparcelCountModel";
        type Type = super::CountModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for CountModel {}

    impl ListModelImpl for CountModel {
        fn item_type(&self) -> glib::Type {
            glib::BoxedAnyObject::static_type()
        }

        fn n_items(&self) -> u32 {
            self.len.get()
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            (position < self.len.get()).then(|| glib::BoxedAnyObject::new(position as usize).upcast())
        }
    }
}

glib::wrapper! {
    /// Rows `0..len`.
    pub struct CountModel(ObjectSubclass<imp::CountModel>) @implements gio::ListModel;
}

impl Default for CountModel {
    fn default() -> CountModel {
        glib::Object::new()
    }
}

impl CountModel {
    /// Returns whether the count changed.
    pub fn set_len(&self, len: usize) -> bool {
        let (old, new) = (self.imp().len.get(), len as u32);
        if old == new {
            return false;
        }
        self.imp().len.set(new);
        if new > old {
            self.items_changed(old, 0, new - old);
        } else {
            self.items_changed(new, old - new, 0);
        }
        true
    }

    /// `removed` rows at `at` replaced by `added` ones (a folder of a tree opened or closed).
    /// Rows after them move, so their cells must read their row from the list item's position.
    pub fn splice(&self, at: usize, removed: usize, added: usize) {
        let len = self.imp().len.get() as usize;
        self.imp().len.set((len - removed + added) as u32);
        self.items_changed(at as u32, removed as u32, added as u32);
    }
}

/// The text of a cell: (row, column).
pub type CellText = Rc<dyn Fn(usize, usize) -> String>;

/// Draws a drawn column's cell: the canvas, the snapshot, its size and the row.
pub type CellPaint = Rc<dyn Fn(&gtk::Widget, &gtk::Snapshot, f32, f32, usize)>;

/// Called for a right click on a row (after it was selected): the widget and where.
pub type ContextMenu = Rc<dyn Fn(&gtk::Widget, f64, f64)>;

/// The state icon before a row's text in the icon column, if any.
pub type CellIcon = Rc<dyn Fn(usize) -> Option<StateIcon>>;

/// A row of a folder tree in the first column ([`Table::with_tree`]): its depth and
/// `Some(expanded)` for a folder, `None` for a file; `None` for a row of a plain list.
pub type CellTree = Rc<dyn Fn(usize) -> Option<(u16, Option<bool>)>>;

/// A click on the expander of the folder in a row.
pub type Expand = Rc<dyn Fn(usize)>;

/// Pixels a tree row's name moves right per level.
const INDENT: i32 = 16;

struct Bound {
    widget: glib::WeakRef<gtk::Widget>,
    /// The list item, whose position is the cell's row: rows after a folder that opened or
    /// closed move ([`CountModel::splice`]) without being bound again.
    item: glib::WeakRef<gtk::ListItem>,
    column: usize,
}

impl Bound {
    fn row(&self) -> Option<usize> {
        let pos = self.item.upgrade()?.position();
        (pos != gtk::INVALID_LIST_POSITION).then_some(pos as usize)
    }
}

/// What the cells of a table draw beyond their text.
#[derive(Clone, Default)]
struct Extras {
    icon: Option<(usize, CellIcon)>,
    tree: Option<CellTree>,
}

pub struct Table {
    pub view: gtk::ColumnView,
    pub model: CountModel,
    pub selection: gtk::MultiSelection,
    bound: Rc<RefCell<Vec<Bound>>>,
    text: CellText,
    extras: Extras,
}

impl Table {
    /// `drawn` is a column drawn by `paint` instead of text.
    pub fn new(
        columns: &[ColumnDef],
        text: CellText,
        drawn: Option<(usize, CellPaint)>,
        menu: Option<ContextMenu>,
    ) -> Table {
        Table::with_icon(columns, text, drawn, menu, None)
    }

    /// As [`new`](Table::new), with column `icon.0` showing `icon.1`'s state icon before its
    /// text.
    pub fn with_icon(
        columns: &[ColumnDef],
        text: CellText,
        drawn: Option<(usize, CellPaint)>,
        menu: Option<ContextMenu>,
        icon: Option<(usize, CellIcon)>,
    ) -> Table {
        Table::build(columns, text, drawn, menu, Extras { icon, tree: None }, None)
    }

    /// As [`new`](Table::new), with the first column drawn as a folder tree while `tree` gives
    /// its rows: the name indented by its depth after an expander arrow (folders only, which
    /// `expand` gets the clicks of) and the theme's folder or file icon. The text is the name.
    pub fn with_tree(
        columns: &[ColumnDef],
        text: CellText,
        menu: Option<ContextMenu>,
        tree: CellTree,
        expand: Expand,
    ) -> Table {
        Table::build(columns, text, None, menu, Extras { icon: None, tree: Some(tree) }, Some(expand))
    }

    fn build(
        columns: &[ColumnDef],
        text: CellText,
        drawn: Option<(usize, CellPaint)>,
        menu: Option<ContextMenu>,
        extras: Extras,
        expand: Option<Expand>,
    ) -> Table {
        let model = CountModel::default();
        let selection = gtk::MultiSelection::new(Some(model.clone()));
        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.set_reorderable(false);
        view.set_show_column_separators(true);
        view.add_css_class("data-table");
        let bound: Rc<RefCell<Vec<Bound>>> = Rc::default();
        let last = columns.len().saturating_sub(1);
        for (column, &(title, width, right)) in columns.iter().enumerate() {
            let paint = drawn.as_ref().filter(|(c, _)| *c == column).map(|(_, p)| p.clone());
            let iconed = extras.icon.as_ref().is_some_and(|(c, _)| *c == column);
            let treed = extras.tree.is_some() && column == 0;
            let factory = gtk::SignalListItemFactory::new();
            {
                let (selection, menu, bound) = (selection.downgrade(), menu.clone(), bound.clone());
                let expand = expand.clone();
                factory.connect_setup(move |_, obj| {
                    let Some(item) = obj.downcast_ref::<gtk::ListItem>() else { return };
                    let widget: gtk::Widget = match &paint {
                        Some(paint) => {
                            let (paint, bound) = (paint.clone(), bound.clone());
                            let canvas = Canvas::new(move |w, s, width, height| {
                                let row = bound
                                    .borrow()
                                    .iter()
                                    .find(|b| b.widget.upgrade().as_ref() == Some(w))
                                    .and_then(Bound::row);
                                if let Some(row) = row {
                                    paint(w, s, width, height, row);
                                }
                            });
                            canvas.set_size_request(-1, 14);
                            canvas.upcast()
                        }
                        None => {
                            let label = gtk::Label::new(None);
                            label.set_xalign(if right { 1.0 } else { 0.0 });
                            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                            label.set_single_line_mode(true);
                            if treed {
                                tree_cell(item, &label, expand.clone()).upcast()
                            } else if iconed {
                                let named = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                                named.append(&crate::icons::state_image());
                                label.set_hexpand(true);
                                named.append(&label);
                                named.upcast()
                            } else {
                                label.upcast()
                            }
                        }
                    };
                    if let Some(menu) = &menu {
                        // a right click selects the row first (unless it is part of the selection)
                        let click = gtk::GestureClick::new();
                        click.set_button(3);
                        let (item_ref, selection, menu) = (item.downgrade(), selection.clone(), menu.clone());
                        click.connect_pressed(move |g, _, x, y| {
                            let (Some(item), Some(selection)) = (item_ref.upgrade(), selection.upgrade())
                            else {
                                return;
                            };
                            let pos = item.position();
                            if pos != gtk::INVALID_LIST_POSITION && !selection.is_selected(pos) {
                                selection.select_item(pos, true);
                            }
                            if let Some(w) = g.widget() {
                                menu(&w, x, y);
                            }
                        });
                        widget.add_controller(click);
                    }
                    item.set_child(Some(&widget));
                    item.connect_position_notify(stripe);
                });
            }
            {
                let (bound, text, extras) = (bound.clone(), text.clone(), extras.clone());
                factory.connect_bind(move |_, obj| {
                    let Some(item) = obj.downcast_ref::<gtk::ListItem>() else { return };
                    let Some(widget) = item.child() else { return };
                    let b = Bound { widget: widget.downgrade(), item: item.downgrade(), column };
                    let row = b.row();
                    bound.borrow_mut().push(b);
                    if let Some(row) = row {
                        draw(&widget, &text, &extras, row, column);
                    }
                    stripe(item);
                });
            }
            {
                let bound = bound.clone();
                factory.connect_unbind(move |_, obj| {
                    let Some(widget) = obj.downcast_ref::<gtk::ListItem>().and_then(|i| i.child()) else {
                        return;
                    };
                    bound.borrow_mut().retain(|b| b.widget.upgrade().is_some_and(|w| w != widget));
                });
            }
            let col = gtk::ColumnViewColumn::new(Some(lang::text(title)), Some(factory));
            col.set_resizable(true);
            col.set_fixed_width(width);
            // the last column takes the rest of the width
            col.set_expand(column == last);
            view.append_column(&col);
        }
        Table { view, model, selection, bound, text, extras }
    }

    /// Sets the row count and redraws the cells on screen.
    pub fn update(&self, rows: usize) {
        self.model.set_len(rows);
        self.redraw();
    }

    /// Redraws the cells on screen.
    pub fn redraw(&self) {
        let bound: Vec<(gtk::Widget, usize, usize)> = self
            .bound
            .borrow()
            .iter()
            .filter_map(|b| Some((b.widget.upgrade()?, b.row()?, b.column)))
            .collect();
        for (widget, row, column) in bound {
            draw(&widget, &self.text, &self.extras, row, column);
        }
    }

    /// The folder in `row` opened or closed, the rows going from `old` to `new`: the rows
    /// below it come or go, and it is the one selected row.
    pub fn spliced(&self, row: usize, old: usize, new: usize) {
        if new > old {
            self.model.splice(row + 1, 0, new - old);
        } else {
            self.model.splice(row + 1, old - new, 0);
        }
        self.selection.select_item(row as u32, true);
        self.redraw();
    }

    /// Every row may be another one now (the tree turned into the plain list or back): the
    /// list starts over with `rows` rows and nothing selected.
    pub fn replace_all(&self, rows: usize) {
        self.selection.unselect_all();
        self.model.splice(0, self.model.n_items() as usize, rows);
        self.redraw();
    }

    /// The selected rows, ascending.
    pub fn selected(&self) -> Vec<usize> {
        let set = self.selection.selection();
        (0..set.size() as u32).map(|i| set.nth(i) as usize).collect()
    }

    pub fn unselect_all(&self) {
        self.selection.unselect_all();
    }

    /// Shows the last row.
    pub fn scroll_to_end(&self) {
        let n = self.model.n_items();
        if n > 0 {
            self.view.scroll_to(n - 1, None, gtk::ListScrollFlags::NONE, None);
        }
    }
}

/// Marks the row holding `item`'s cell `pa-alt` when it is an odd one, for the alternate row
/// shading (`ui.alternate_rows`, the rule in `app::STRIPES`). Called as the cell is bound and
/// whenever rows before it come or go; a CSS `nth-child` would count only the rows on screen.
pub fn stripe(item: &gtk::ListItem) {
    let Some(child) = item.child() else { return };
    // A cell is set up and bound while GTK still constructs it and its row. Their parents are
    // floating references then, and taking one (`parent()`) would sink it and drop it with
    // this function's reference: the cell (or row) is destroyed mid-construction, which
    // aborts in gtklistfactorywidget.c (or leaves the list without rows). The parents are
    // safe to take once the cell is in a window: shade the row in the next idle instead.
    if child.root().is_none() {
        let item = item.downgrade();
        glib::idle_add_local_once(move || {
            let Some(item) = item.upgrade() else { return };
            if item.child().is_some_and(|c| c.root().is_some()) {
                stripe(&item);
            }
        });
        return;
    }
    // the child's parent is its cell, the cell's parent the row
    let Some(row) = child.parent().and_then(|c| c.parent()) else { return };
    if row.css_name() != "row" {
        return;
    }
    let pos = item.position();
    if pos != gtk::INVALID_LIST_POSITION && pos % 2 == 1 {
        row.add_css_class("pa-alt");
    } else {
        row.remove_css_class("pa-alt");
    }
}

fn set_text(label: &gtk::Label, t: &str) {
    if label.text() != t {
        label.set_text(t);
    }
}

/// The first column's cell of a tree: the expander (a flat button whose clicks go to `expand`
/// with the item's row), the icon and `label`.
fn tree_cell(item: &gtk::ListItem, label: &gtk::Label, expand: Option<Expand>) -> gtk::Box {
    let expander = gtk::Button::from_icon_name("pan-end-symbolic");
    expander.add_css_class("flat");
    expander.add_css_class("pa-expander");
    expander.set_focus_on_click(false);
    expander.set_valign(gtk::Align::Center);
    let item_ref = item.downgrade();
    expander.connect_clicked(move |_| {
        let (Some(item), Some(expand)) = (item_ref.upgrade(), &expand) else { return };
        let pos = item.position();
        if pos != gtk::INVALID_LIST_POSITION {
            expand(pos as usize);
        }
    });
    let image = gtk::Image::new();
    label.set_hexpand(true);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    b.append(&expander);
    b.append(&image);
    b.append(label);
    b
}

/// Draws a [`tree_cell`]: indented by the row's depth, the expander shown for folders (and
/// kept invisible for files, so names line up), the folder or file icon; neither expander nor
/// icon in a plain list.
fn draw_tree(cell: &gtk::Box, tree: &CellTree, name: &str, row: usize) {
    let Some(expander) = cell.first_child().and_downcast::<gtk::Button>() else { return };
    let Some(image) = expander.next_sibling().and_downcast::<gtk::Image>() else { return };
    let Some((depth, folder)) = tree(row) else {
        expander.set_visible(false);
        image.set_visible(false);
        return;
    };
    expander.set_visible(true);
    image.set_visible(true);
    expander.set_margin_start(i32::from(depth) * INDENT);
    expander.set_opacity(if folder.is_some() { 1.0 } else { 0.0 });
    expander.set_can_target(folder.is_some());
    let arrow = match folder {
        Some(true) => "pan-down-symbolic",
        _ if lang::right_to_left() => "pan-start-symbolic",
        _ => "pan-end-symbolic",
    };
    if expander.icon_name().as_deref() != Some(arrow) {
        expander.set_icon_name(arrow);
    }
    image.set_from_gicon(&crate::icons::file_icon(name, folder));
}

fn draw(widget: &gtk::Widget, text: &CellText, extras: &Extras, row: usize, column: usize) {
    if let Some(l) = widget.downcast_ref::<gtk::Label>() {
        set_text(l, &text(row, column));
    } else if let Some(named) = widget.downcast_ref::<gtk::Box>() {
        if let (Some(tree), 0) = (&extras.tree, column) {
            let name = text(row, column);
            draw_tree(named, tree, &name, row);
            if let Some(l) = named.last_child().and_downcast::<gtk::Label>() {
                set_text(&l, &name);
            }
            return;
        }
        if let (Some(image), Some((_, icon))) =
            (named.first_child().and_downcast::<gtk::Image>(), extras.icon.as_ref())
        {
            match icon(row) {
                Some(state) => crate::icons::set_state(&image, state),
                None => image.clear(),
            }
        }
        if let Some(l) = named.last_child().and_downcast::<gtk::Label>() {
            set_text(&l, &text(row, column));
        }
    } else {
        widget.queue_draw();
    }
}
