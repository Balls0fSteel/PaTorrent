//! The category list on the left, µTorrent 2.2.1's tree as a flat list: Torrents, Downloading,
//! Completed, Active and Inactive with their counts, then the Labels: "No Label" and every
//! label in use, as the Win32 tree shows them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use patorrent_frontend::model::{Filter, TorrentList};
use patorrent_frontend::status::Category;

pub struct Sidebar {
    pub widget: gtk::ListBox,
    /// The filter of each selectable row, by row index (`None` for the Labels heading).
    rows: RefCell<Vec<(Option<Filter>, gtk::Label)>>,
    /// The labels the rows were built for.
    labels: RefCell<Vec<String>>,
    /// Rows are being rebuilt: their selection changes are not the user's.
    rebuilding: Cell<bool>,
}

impl Sidebar {
    /// `on_select` gets the filter of each row the user picks.
    pub fn new(on_select: impl Fn(Filter) + 'static) -> Rc<Sidebar> {
        let widget = gtk::ListBox::new();
        widget.set_selection_mode(gtk::SelectionMode::Single);
        widget.add_css_class("navigation-sidebar");
        let sidebar = Rc::new(Sidebar {
            widget,
            rows: RefCell::default(),
            labels: RefCell::default(),
            rebuilding: Cell::new(false),
        });
        let weak = Rc::downgrade(&sidebar);
        sidebar.widget.connect_row_selected(move |_, row| {
            let (Some(s), Some(row)) = (weak.upgrade(), row) else { return };
            if s.rebuilding.get() {
                return;
            }
            let filter = s.rows.borrow().get(row.index() as usize).and_then(|(f, _)| f.clone());
            if let Some(f) = filter {
                on_select(f);
            }
        });
        sidebar.rebuild(&[], &Filter::default());
        sidebar
    }

    /// Updates the counts; rebuilds the label rows when the labels in use changed.
    pub fn update(&self, list: &TorrentList) {
        let labels = list.labels();
        let names: Vec<String> = labels.iter().map(|(l, _)| l.clone()).collect();
        if *self.labels.borrow() != names {
            self.rebuild(&names, list.filter());
        }
        let labelled: usize = labels.iter().map(|(_, n)| n).sum();
        for (filter, label) in self.rows.borrow().iter() {
            let text = match filter {
                Some(Filter::Category(c)) => format!("{} ({})", c.label(), list.count(*c)),
                Some(Filter::Label(l)) if l.is_empty() => format!("No Label ({})", list.len() - labelled),
                Some(Filter::Label(l)) => {
                    let n = labels.iter().find(|(x, _)| x == l).map_or(0, |(_, n)| *n);
                    format!("{l} ({n})")
                }
                None => continue,
            };
            if label.text() != text {
                label.set_text(&text);
            }
        }
    }

    fn rebuild(&self, labels: &[String], selected: &Filter) {
        self.rebuilding.set(true);
        while let Some(row) = self.widget.row_at_index(0) {
            self.widget.remove(&row);
        }
        let mut rows = Vec::new();
        let mut add = |filter: Option<Filter>, text: &str, indent: bool| {
            let label = gtk::Label::new(Some(text));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            if indent {
                label.set_margin_start(14);
            }
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&label));
            if filter.is_none() {
                row.set_selectable(false);
                row.set_activatable(false);
                label.add_css_class("heading");
                label.set_margin_top(8);
            }
            self.widget.append(&row);
            rows.push((filter, label));
        };
        for (i, c) in Category::ALL.iter().enumerate() {
            add(Some(Filter::Category(*c)), c.label(), i > 0);
        }
        add(None, "Labels", false);
        add(Some(Filter::Label(String::new())), "No Label", true);
        for l in labels {
            add(Some(Filter::Label(l.clone())), l, true);
        }
        let index = rows.iter().position(|(f, _)| f.as_ref() == Some(selected)).unwrap_or(0);
        *self.rows.borrow_mut() = rows;
        *self.labels.borrow_mut() = labels.to_vec();
        self.widget.select_row(self.widget.row_at_index(index as i32).as_ref());
        self.rebuilding.set(false);
    }
}
