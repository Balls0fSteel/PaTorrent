//! The torrent list: a `GtkColumnView` over [`RowModel`], a `GListModel` of the frontend
//! model's view (`TorrentList::view`, already filtered and sorted). It hands out one small
//! object per row only when GTK asks (the rows on screen and a few more), so there is no object
//! per torrent (docs/ARCHITECTURE.md "GUI list").
//!
//! Cells are drawn from the frontend's columns by a render callback. GTK binds only the
//! visible cells; they are remembered, so the 1 Hz refresh redraws those and nothing else.
//! When the order or the set of rows changes the model is replaced (`items_changed`), and the
//! caller puts the selection back by torrent id, as the Win32 list does.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use patorrent_core::TorrentId;
use patorrent_frontend::columns::{Align, Column};
use patorrent_frontend::icons::StateIcon;

mod imp {
    use std::cell::RefCell;

    use gtk::subclass::prelude::*;
    use gtk::{gio, glib, prelude::*};

    #[derive(Default)]
    pub struct RowModel {
        /// Torrent ids (raw), in display order.
        pub ids: RefCell<Vec<u64>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RowModel {
        const NAME: &'static str = "PaTorrentRowModel";
        type Type = super::RowModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for RowModel {}

    impl ListModelImpl for RowModel {
        fn item_type(&self) -> glib::Type {
            glib::BoxedAnyObject::static_type()
        }

        fn n_items(&self) -> u32 {
            self.ids.borrow().len() as u32
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let id = *self.ids.borrow().get(position as usize)?;
            Some(glib::BoxedAnyObject::new(id).upcast())
        }
    }
}

glib::wrapper! {
    /// The rows: torrent ids in display order.
    pub struct RowModel(ObjectSubclass<imp::RowModel>) @implements gio::ListModel;
}

impl Default for RowModel {
    fn default() -> RowModel {
        glib::Object::new()
    }
}

impl RowModel {
    /// Replaces the rows; tells the view when they differ. Returns whether they did.
    pub fn set_ids(&self, ids: Vec<u64>) -> bool {
        let imp = self.imp();
        if *imp.ids.borrow() == ids {
            return false;
        }
        let (old, new) = (imp.ids.borrow().len() as u32, ids.len() as u32);
        *imp.ids.borrow_mut() = ids;
        self.items_changed(0, old, new);
        true
    }

    pub fn id_at(&self, position: u32) -> Option<TorrentId> {
        self.imp().ids.borrow().get(position as usize).map(|&id| TorrentId::from_raw(id))
    }

    fn positions(&self) -> HashMap<u64, u32> {
        self.imp().ids.borrow().iter().enumerate().map(|(i, &id)| (id, i as u32)).collect()
    }
}

/// What a cell shows.
pub enum Content {
    Text(String),
    /// The Name column: the state icon and the text.
    Named(StateIcon, String),
    /// The Done column: a bar with its text.
    Progress(f64, String),
}

/// Draws a cell: the torrent and the column. `None` when the torrent is gone.
pub type Render = Rc<dyn Fn(TorrentId, Column) -> Option<Content>>;

/// Called for a right click on a row (after it was selected): the widget and where.
pub type ContextMenu = Rc<dyn Fn(&gtk::Widget, f64, f64)>;

/// A bound cell: its widget, column and torrent.
struct Bound {
    widget: glib::WeakRef<gtk::Widget>,
    column: Column,
    id: TorrentId,
}

pub struct List {
    pub view: gtk::ColumnView,
    pub model: RowModel,
    pub selection: gtk::MultiSelection,
    /// Every column, by identity.
    pub columns: Vec<(Column, gtk::ColumnViewColumn)>,
    bound: Rc<RefCell<Vec<Bound>>>,
    render: Render,
}

impl List {
    /// The view with every column in `layout` order (width in pixels, negative = hidden),
    /// sorted by `sort` (the indicator only: the model is sorted by the frontend).
    pub fn new(
        layout: &[(Column, i32)],
        sort: Option<(Column, bool)>,
        render: Render,
        menu: ContextMenu,
    ) -> List {
        let model = RowModel::default();
        let selection = gtk::MultiSelection::new(Some(model.clone()));
        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.set_reorderable(true);
        view.set_show_column_separators(true);
        view.add_css_class("data-table");
        let bound: Rc<RefCell<Vec<Bound>>> = Rc::default();
        let mut columns = Vec::new();
        for &(column, width) in layout {
            let factory = gtk::SignalListItemFactory::new();
            {
                let (selection, menu) = (selection.downgrade(), menu.clone());
                factory.connect_setup(move |_, obj| {
                    let Some(item) = obj.downcast_ref::<gtk::ListItem>() else { return };
                    let widget = cell_widget(column);
                    // a right click selects the row first (unless it is part of the selection)
                    let click = gtk::GestureClick::new();
                    click.set_button(3);
                    let (item_ref, selection, menu) = (item.downgrade(), selection.clone(), menu.clone());
                    click.connect_pressed(move |g, _, x, y| {
                        let (Some(item), Some(selection)) = (item_ref.upgrade(), selection.upgrade()) else {
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
                    item.set_child(Some(&widget));
                });
            }
            {
                let (bound, render) = (bound.clone(), render.clone());
                factory.connect_bind(move |_, obj| {
                    let Some(item) = obj.downcast_ref::<gtk::ListItem>() else { return };
                    let (Some(widget), Some(row)) =
                        (item.child(), item.item().and_downcast::<glib::BoxedAnyObject>())
                    else {
                        return;
                    };
                    let id = TorrentId::from_raw(*row.borrow::<u64>());
                    if let Some(c) = render(id, column) {
                        draw(&widget, c);
                    }
                    bound.borrow_mut().push(Bound { widget: widget.downgrade(), column, id });
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
            let col = gtk::ColumnViewColumn::new(Some(column.title()), Some(factory));
            col.set_id(Some(column.key()));
            col.set_resizable(true);
            col.set_fixed_width(width.abs().max(16));
            col.set_visible(width > 0);
            col.set_sorter(Some(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal)));
            view.append_column(&col);
            columns.push((column, col));
        }
        if let Some((column, descending)) = sort {
            if let Some((_, col)) = columns.iter().find(|(c, _)| *c == column) {
                let order = if descending { gtk::SortType::Descending } else { gtk::SortType::Ascending };
                view.sort_by_column(Some(col), order);
            }
        }
        List { view, model, selection, columns, bound, render }
    }

    /// Redraws the cells on screen.
    pub fn redraw(&self) {
        let bound: Vec<(gtk::Widget, Column, TorrentId)> = self
            .bound
            .borrow()
            .iter()
            .filter_map(|b| b.widget.upgrade().map(|w| (w, b.column, b.id)))
            .collect();
        for (widget, column, id) in bound {
            if let Some(c) = (self.render)(id, column) {
                draw(&widget, c);
            }
        }
    }

    /// The selected torrents, in display order.
    pub fn selected(&self) -> Vec<TorrentId> {
        let set = self.selection.selection();
        (0..set.size() as u32).filter_map(|i| self.model.id_at(set.nth(i))).collect()
    }

    /// Selects exactly `ids` (those still in the list).
    pub fn select(&self, ids: &[TorrentId]) {
        let positions = self.model.positions();
        let set = gtk::Bitset::new_empty();
        for id in ids {
            if let Some(&p) = positions.get(&id.to_raw()) {
                set.add(p);
            }
        }
        let n = self.model.n_items();
        let mask = gtk::Bitset::new_range(0, n);
        self.selection.set_selection(&set, &mask);
    }

    /// The columns in display order with their widths (negative = hidden), for the settings.
    pub fn layout(&self) -> Vec<(Column, i32)> {
        let shown = self.view.columns();
        let mut out = Vec::new();
        for i in 0..shown.n_items() {
            let Some(col) = shown.item(i).and_downcast::<gtk::ColumnViewColumn>() else { continue };
            let Some(&(column, _)) = self.columns.iter().find(|(_, c)| *c == col) else { continue };
            let w = if col.fixed_width() > 0 { col.fixed_width() } else { column.default_width() };
            out.push((column, if col.is_visible() { w } else { -w }));
        }
        out
    }

    /// The column and direction the header shows.
    pub fn sort(&self) -> Option<(Column, bool)> {
        let sorter = self.view.sorter().and_downcast::<gtk::ColumnViewSorter>()?;
        let col = sorter.primary_sort_column()?;
        let &(column, _) = self.columns.iter().find(|(_, c)| *c == col)?;
        Some((column, sorter.primary_sort_order() == gtk::SortType::Descending))
    }
}

fn cell_widget(column: Column) -> gtk::Widget {
    if column == Column::Done {
        // µTorrent's bar with the percentage on it: GTK draws a progress bar's own text beside
        // the bar, so the text is a label over it
        let bar = gtk::ProgressBar::new();
        bar.set_valign(gtk::Align::Center);
        bar.add_css_class("pa-done");
        let text = gtk::Label::new(None);
        text.add_css_class("pa-done-text");
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&bar));
        overlay.add_overlay(&text);
        return overlay.upcast();
    }
    let label = gtk::Label::new(None);
    label.set_xalign(if column.align() == Align::Right { 1.0 } else { 0.0 });
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_single_line_mode(true);
    if column == Column::Name {
        let named = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        named.append(&crate::icons::state_image());
        label.set_hexpand(true);
        named.append(&label);
        return named.upcast();
    }
    label.upcast()
}

fn set_text(label: &gtk::Label, t: &str) {
    if label.text() != t {
        label.set_text(t);
    }
}

fn draw(widget: &gtk::Widget, content: Content) {
    match content {
        Content::Text(t) => {
            if let Some(l) = widget.downcast_ref::<gtk::Label>() {
                set_text(l, &t);
            }
        }
        Content::Named(icon, t) => {
            let Some(named) = widget.downcast_ref::<gtk::Box>() else { return };
            if let Some(image) = named.first_child().and_downcast::<gtk::Image>() {
                crate::icons::set_state(&image, icon);
            }
            if let Some(l) = named.last_child().and_downcast::<gtk::Label>() {
                set_text(&l, &t);
            }
        }
        Content::Progress(fraction, t) => {
            let Some(overlay) = widget.downcast_ref::<gtk::Overlay>() else { return };
            if let Some(p) = overlay.child().and_downcast::<gtk::ProgressBar>() {
                p.set_fraction(fraction.clamp(0.0, 1.0));
            }
            if let Some(l) = overlay.last_child().and_downcast::<gtk::Label>() {
                set_text(&l, &t);
            }
        }
    }
}
