//! The detail tabs' tables (Trackers, Peers, Files, Logger and the Pieces list): a
//! `GtkColumnView` over [`CountModel`], a `GListModel` that is only a row count. Rows are
//! indices into the frontend's [`Detail`](patorrent_frontend::detail::Detail); the cells
//! on screen are remembered and redrawn from it, as the torrent list does.
//!
//! A count that grows adds rows at the end and one that shrinks drops them there, so the
//! Logger keeps its scroll position while lines arrive.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use patorrent_frontend::detail::ColumnDef;
use patorrent_frontend::lang;

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
        const NAME: &'static str = "PaTorrentCountModel";
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
}

/// The text of a cell: (row, column).
pub type CellText = Rc<dyn Fn(usize, usize) -> String>;

/// Draws a drawn column's cell: the canvas, the snapshot, its size and the row.
pub type CellPaint = Rc<dyn Fn(&gtk::Widget, &gtk::Snapshot, f32, f32, usize)>;

/// Called for a right click on a row (after it was selected): the widget and where.
pub type ContextMenu = Rc<dyn Fn(&gtk::Widget, f64, f64)>;

struct Bound {
    widget: glib::WeakRef<gtk::Widget>,
    column: usize,
    row: usize,
}

pub struct Table {
    pub view: gtk::ColumnView,
    pub model: CountModel,
    pub selection: gtk::MultiSelection,
    bound: Rc<RefCell<Vec<Bound>>>,
    text: CellText,
}

impl Table {
    /// `drawn` is a column drawn by `paint` instead of text.
    pub fn new(
        columns: &[ColumnDef],
        text: CellText,
        drawn: Option<(usize, CellPaint)>,
        menu: Option<ContextMenu>,
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
            let factory = gtk::SignalListItemFactory::new();
            {
                let (selection, menu, bound) = (selection.downgrade(), menu.clone(), bound.clone());
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
                                    .map(|b| b.row);
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
                            label.upcast()
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
                });
            }
            {
                let (bound, text) = (bound.clone(), text.clone());
                factory.connect_bind(move |_, obj| {
                    let Some(item) = obj.downcast_ref::<gtk::ListItem>() else { return };
                    let (Some(widget), Some(row)) =
                        (item.child(), item.item().and_downcast::<glib::BoxedAnyObject>())
                    else {
                        return;
                    };
                    let row = *row.borrow::<usize>();
                    bound.borrow_mut().push(Bound { widget: widget.downgrade(), column, row });
                    draw(&widget, &text, row, column);
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
        Table { view, model, selection, bound, text }
    }

    /// Sets the row count and redraws the cells on screen.
    pub fn update(&self, rows: usize) {
        self.model.set_len(rows);
        let bound: Vec<(gtk::Widget, usize, usize)> = self
            .bound
            .borrow()
            .iter()
            .filter_map(|b| b.widget.upgrade().map(|w| (w, b.row, b.column)))
            .collect();
        for (widget, row, column) in bound {
            draw(&widget, &self.text, row, column);
        }
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

fn draw(widget: &gtk::Widget, text: &CellText, row: usize, column: usize) {
    if let Some(l) = widget.downcast_ref::<gtk::Label>() {
        let t = text(row, column);
        if l.text() != t {
            l.set_text(&t);
        }
    } else {
        widget.queue_draw();
    }
}
