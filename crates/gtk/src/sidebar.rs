//! The category list on the left, µTorrent 2.2.1's tree as a flat list: Torrents, Downloading,
//! Completed, Active and Inactive with their counts, then the Labels: "No Label" and every
//! label in use, then the Feeds (all feeds' items, then each feed's), as the Win32 tree shows
//! them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use patorrent_frontend::model::{Filter, TorrentList};
use patorrent_frontend::rss::FeedId;
use patorrent_frontend::status::Category;
use patorrent_frontend::tr;

/// What a row shows in the main list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Torrents(Filter),
    /// A feed's items (`None`: every feed's), µTorrent's way of listing them.
    Feed(Option<FeedId>),
}

/// Called for a right click on a feed row (after it was picked): the feed (`None`: the Feeds
/// row), the widget and where.
pub type FeedMenu = Rc<dyn Fn(Option<FeedId>, &gtk::Widget, f64, f64)>;

pub struct Sidebar {
    pub widget: gtk::ListBox,
    /// What each row picks, by row index (`None` for the Labels heading).
    rows: RefCell<Vec<(Option<Pick>, gtk::Label)>>,
    /// The labels and feeds the rows were built for.
    labels: RefCell<Vec<String>>,
    feeds: RefCell<Vec<(FeedId, String)>>,
    /// The row picked last.
    current: RefCell<Pick>,
    /// Rows are being rebuilt: their selection changes are not the user's.
    rebuilding: Cell<bool>,
}

impl Sidebar {
    /// `on_select` gets what each row the user picks shows; `feed_menu` the right clicks on the
    /// Feeds rows.
    pub fn new(on_select: impl Fn(Pick) + 'static, feed_menu: FeedMenu) -> Rc<Sidebar> {
        let widget = gtk::ListBox::new();
        widget.set_selection_mode(gtk::SelectionMode::Single);
        widget.add_css_class("navigation-sidebar");
        let sidebar = Rc::new(Sidebar {
            widget,
            rows: RefCell::default(),
            labels: RefCell::default(),
            feeds: RefCell::default(),
            current: RefCell::new(Pick::Torrents(Filter::default())),
            rebuilding: Cell::new(false),
        });
        let weak = Rc::downgrade(&sidebar);
        sidebar.widget.connect_row_selected(move |_, row| {
            let (Some(s), Some(row)) = (weak.upgrade(), row) else { return };
            if s.rebuilding.get() {
                return;
            }
            let pick = s.rows.borrow().get(row.index() as usize).and_then(|(p, _)| p.clone());
            if let Some(p) = pick {
                *s.current.borrow_mut() = p.clone();
                on_select(p);
            }
        });
        let click = gtk::GestureClick::new();
        click.set_button(3);
        let weak = Rc::downgrade(&sidebar);
        click.connect_pressed(move |g, _, x, y| {
            let Some(s) = weak.upgrade() else { return };
            let Some(row) = s.widget.row_at_y(y as i32) else { return };
            let pick = s.rows.borrow().get(row.index() as usize).and_then(|(p, _)| p.clone());
            let Some(Pick::Feed(feed)) = pick else { return };
            s.widget.select_row(Some(&row));
            if let Some(w) = g.widget() {
                feed_menu(feed, &w, x, y);
            }
        });
        sidebar.widget.add_controller(click);
        sidebar.rebuild(&[], &[]);
        sidebar
    }

    /// What the list shows now.
    pub fn current(&self) -> Pick {
        self.current.borrow().clone()
    }

    /// Updates the counts; rebuilds the rows when the labels in use or the feeds changed.
    pub fn update(&self, list: &TorrentList, feeds: &[(FeedId, String)]) {
        let labels = list.labels();
        let names: Vec<String> = labels.iter().map(|(l, _)| l.clone()).collect();
        if *self.labels.borrow() != names || *self.feeds.borrow() != feeds {
            self.rebuild(&names, feeds);
        }
        let labelled: usize = labels.iter().map(|(_, n)| n).sum();
        for (pick, label) in self.rows.borrow().iter() {
            let text = match pick {
                Some(Pick::Torrents(Filter::Category(c))) => format!("{} ({})", c.label(), list.count(*c)),
                Some(Pick::Torrents(Filter::Label(l))) if l.is_empty() => {
                    format!("{} ({})", tr!(CatNoLabel), list.len() - labelled)
                }
                Some(Pick::Torrents(Filter::Label(l))) => {
                    let n = labels.iter().find(|(x, _)| x == l).map_or(0, |(_, n)| *n);
                    format!("{l} ({n})")
                }
                _ => continue,
            };
            if label.text() != text {
                label.set_text(&text);
            }
        }
    }

    fn rebuild(&self, labels: &[String], feeds: &[(FeedId, String)]) {
        self.rebuilding.set(true);
        while let Some(row) = self.widget.row_at_index(0) {
            self.widget.remove(&row);
        }
        let mut rows = Vec::new();
        let mut add = |pick: Option<Pick>, text: &str, indent: bool, heading: bool| {
            let label = gtk::Label::new(Some(text));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            if indent {
                label.set_margin_start(14);
            }
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&label));
            if heading {
                label.add_css_class("heading");
                label.set_margin_top(8);
            }
            if pick.is_none() {
                row.set_selectable(false);
                row.set_activatable(false);
            }
            self.widget.append(&row);
            rows.push((pick, label));
        };
        for (i, c) in Category::ALL.iter().enumerate() {
            add(Some(Pick::Torrents(Filter::Category(*c))), c.label(), i > 0, false);
        }
        add(None, tr!(CatLabels), false, true);
        add(Some(Pick::Torrents(Filter::Label(String::new()))), tr!(CatNoLabel), true, false);
        for l in labels {
            add(Some(Pick::Torrents(Filter::Label(l.clone()))), l, true, false);
        }
        // the Feeds row lists every feed's items, as the Win32 tree's Feeds node
        add(Some(Pick::Feed(None)), tr!(CatFeeds), false, true);
        for (id, name) in feeds {
            add(Some(Pick::Feed(Some(*id))), name, true, false);
        }
        let current = self.current.borrow().clone();
        let index = rows.iter().position(|(p, _)| p.as_ref() == Some(&current));
        if index.is_none() {
            // what was shown went away
            *self.current.borrow_mut() = Pick::Torrents(Filter::default());
        }
        *self.rows.borrow_mut() = rows;
        *self.labels.borrow_mut() = labels.to_vec();
        *self.feeds.borrow_mut() = feeds.to_vec();
        self.widget.select_row(self.widget.row_at_index(index.unwrap_or(0) as i32).as_ref());
        self.rebuilding.set(false);
    }
}
