//! The RSS Downloader window (µTorrent 2.2.1: Options > RSS Downloader and the toolbar's RSS
//! button) and its Add RSS Feed dialog, on [`patorrent_frontend::rss`], as the Win32 client's.
//!
//! Left, the Feeds list (All Feeds, then each feed) with Add, Edit, Remove and Update and the
//! selected feed's state under it. Right, two tabs: Filters (the filter list with check boxes
//! for on and off, the selected filter's fields, and the items of the feeds' last fetches it
//! matches) and History (what the downloader added). Edits go to the live downloader at once
//! through [`Host`] (the window has Close rather than OK and Cancel); the owner runs the
//! filters over the feeds' current items when it closes ([`show`]'s `closed`).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_frontend::lang::{self, S, gtk_mnemonic};
use patorrent_frontend::rss::view::{self, FeedForm, quality_text};
use patorrent_frontend::rss::{FeedId, FilterId, MIN_INTERVALS, Quality, RssDownloader, RssFilter};
use patorrent_frontend::tr;

use crate::table::Table;

/// What the window needs of its owner.
pub trait Host {
    /// Runs `f` on the live downloader to read it.
    fn view(&self, f: &mut dyn FnMut(&RssDownloader));
    /// Runs `f` on the live downloader to change it (the owner saves `rss.dat`).
    fn edit(&self, f: &mut dyn FnMut(&mut RssDownloader));
    /// Whether feed `id` is being fetched.
    fn updating(&self, id: FeedId) -> bool;
    /// "Update": fetch feed `id` now (`None`: every feed).
    fn update(&self, id: Option<FeedId>);
    /// The labels in use, for the Label field.
    fn labels(&self) -> Vec<String>;
}

/// Reads the downloader through `host`.
fn read<R>(host: &dyn Host, f: impl FnOnce(&RssDownloader) -> R) -> Option<R> {
    let mut f = Some(f);
    let mut out = None;
    host.view(&mut |d| {
        if let Some(f) = f.take() {
            out = Some(f(d));
        }
    });
    out
}

fn edit(host: &dyn Host, f: impl FnOnce(&mut RssDownloader)) {
    let mut f = Some(f);
    host.edit(&mut |d| {
        if let Some(f) = f.take() {
            f(d);
        }
    });
}

fn margins(w: &impl IsA<gtk::Widget>, m: i32) {
    w.set_margin_top(m);
    w.set_margin_bottom(m);
    w.set_margin_start(m);
    w.set_margin_end(m);
}

fn escape_closes(window: &gtk::Window) {
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);
}

fn message(parent: Option<&gtk::Window>, text: &str) {
    gtk::AlertDialog::builder().message(text).modal(true).build().show(parent);
}

// ---- the Add RSS Feed dialog -------------------------------------------------------------

/// Shows the Add RSS Feed dialog (`title` says which) over `parent`; `done` gets the fields on
/// OK.
pub fn feed_dialog(parent: &gtk::Window, title: &str, form: FeedForm, done: impl Fn(FeedForm) + 'static) {
    let window = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(true)
        .default_width(440)
        .resizable(false)
        .build();
    let caption = |t: &str, w: &gtk::Entry| {
        let l = gtk::Label::with_mnemonic(&gtk_mnemonic(t));
        l.set_xalign(0.0);
        l.set_mnemonic_widget(Some(w));
        l
    };
    let url = gtk::Entry::new();
    url.set_text(&form.url);
    url.set_activates_default(true);
    let alias = gtk::Entry::new();
    alias.set_text(&form.alias);
    alias.set_activates_default(true);
    let no_auto = gtk::CheckButton::with_mnemonic(&gtk_mnemonic(tr!(RssNoAuto)));
    let auto = gtk::CheckButton::with_mnemonic(&gtk_mnemonic(tr!(RssAuto)));
    auto.set_group(Some(&no_auto));
    let smart = gtk::CheckButton::with_mnemonic(&gtk_mnemonic(tr!(RssSmartAuto)));
    smart.set_margin_start(24);
    if form.subscribe { &auto } else { &no_auto }.set_active(true);
    smart.set_active(form.smart);
    smart.set_sensitive(form.subscribe);
    let s = smart.downgrade();
    auto.connect_toggled(move |a| {
        if let Some(s) = s.upgrade() {
            s.set_sensitive(a.is_active());
        }
    });
    let subscription = gtk::Box::new(gtk::Orientation::Vertical, 4);
    margins(&subscription, 8);
    subscription.append(&no_auto);
    subscription.append(&auto);
    subscription.append(&smart);
    let frame = gtk::Frame::new(Some(&lang::plain(tr!(RssSubscription))));
    frame.set_child(Some(&subscription));

    let ok = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnOk)));
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnCancel)));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&ok);
    buttons.append(&cancel);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    margins(&body, 12);
    body.append(&caption(tr!(RssFeedUrl), &url));
    body.append(&url);
    body.append(&caption(tr!(RssAlias), &alias));
    body.append(&alias);
    frame.set_margin_top(6);
    body.append(&frame);
    buttons.set_margin_top(6);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&ok));
    escape_closes(&window);
    let w = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    let (w, url_w, alias_w, auto_w, smart_w) =
        (window.downgrade(), url.downgrade(), alias.downgrade(), auto.downgrade(), smart.downgrade());
    ok.connect_clicked(move |_| {
        let (Some(w), Some(url), Some(alias), Some(auto), Some(smart)) =
            (w.upgrade(), url_w.upgrade(), alias_w.upgrade(), auto_w.upgrade(), smart_w.upgrade())
        else {
            return;
        };
        let u = url.text().trim().to_owned();
        if u.is_empty() {
            message(Some(&w), tr!(RssNoUrl));
            url.grab_focus();
            return;
        }
        let subscribe = auto.is_active();
        let form = FeedForm {
            url: u,
            alias: alias.text().trim().to_owned(),
            subscribe,
            smart: subscribe && smart.is_active(),
        };
        w.close();
        done(form);
    });
    window.present();
    url.grab_focus();
}

/// The Add RSS Feed dialog over `parent` for a new feed, its URL from the clipboard when that
/// holds one; `done` gets the fields on OK.
pub fn add_feed_dialog(parent: &gtk::Window, done: impl Fn(FeedForm) + 'static) {
    let parent = parent.clone();
    parent.clipboard().read_text_async(None::<&gio::Cancellable>, {
        let parent = parent.clone();
        move |r| {
            let url = r.ok().flatten().and_then(|t| view::feed_url(&t)).unwrap_or_default();
            feed_dialog(&parent, tr!(RssAddFeed), FeedForm { url, ..FeedForm::default() }, done);
        }
    });
}

// ---- the RSS Downloader window -----------------------------------------------------------

/// Most rows the preview shows.
const PREVIEW_ROWS: usize = 500;

struct Dialog {
    host: Rc<dyn Host>,
    window: glib::WeakRef<gtk::Window>,
    /// The Feeds list and its rows (`None`: All Feeds).
    feeds: gtk::ListBox,
    feed_rows: RefCell<Vec<Option<FeedId>>>,
    feed_status: gtk::Label,
    feed_edit: gtk::Button,
    feed_remove: gtk::Button,
    /// The filter list, its rows and the filter whose fields show.
    filters: gtk::ListBox,
    filter_rows: RefCell<Vec<FilterId>>,
    filter: Cell<Option<FilterId>>,
    name: gtk::Entry,
    pattern: gtk::Entry,
    not: gtk::Entry,
    save: gtk::Entry,
    save_browse: gtk::Button,
    /// The Feed field and its choices (`None`: All Feeds).
    feed: gtk::DropDown,
    feed_choices: RefCell<Vec<Option<FeedId>>>,
    quality: gtk::MenuButton,
    /// "All", then one per `Quality::LIST` entry.
    quality_checks: Vec<gtk::CheckButton>,
    episodes: gtk::Entry,
    interval: gtk::DropDown,
    label: gtk::Entry,
    label_row: gtk::Box,
    smart: gtk::CheckButton,
    priority: gtk::CheckButton,
    paused: gtk::CheckButton,
    filter_remove: gtk::Button,
    filter_up: gtk::Button,
    filter_down: gtk::Button,
    preview: Table,
    preview_rows: Rc<RefCell<Vec<[String; 2]>>>,
    history: Table,
    history_rows: Rc<RefCell<Vec<[String; 3]>>>,
    /// Controls are being filled: their signals are not edits.
    loading: Cell<bool>,
}

thread_local! {
    /// The open window, for [`changed`].
    static OPEN: RefCell<Option<Weak<Dialog>>> = const { RefCell::new(None) };
}

/// The downloader changed underneath (a fetch finished): the open window shows it again.
pub fn changed() {
    if let Some(d) = OPEN.with(|o| o.borrow().as_ref().and_then(Weak::upgrade)) {
        d.refresh();
    }
}

/// Shows the window over `parent` with `feed` picked in the Feeds list; `closed` runs when it
/// closes.
pub fn show(
    parent: &gtk::ApplicationWindow,
    host: Rc<dyn Host>,
    feed: Option<FeedId>,
    closed: impl Fn() + 'static,
) {
    if let Some(d) = OPEN.with(|o| o.borrow().as_ref().and_then(Weak::upgrade)) {
        if let Some(w) = d.window.upgrade() {
            w.present();
        }
        return;
    }
    let window = gtk::Window::builder()
        .title(tr!(RssTitle))
        .transient_for(parent)
        .modal(true)
        .default_width(920)
        .default_height(620)
        .build();
    let button = |t: &str| gtk::Button::with_mnemonic(&gtk_mnemonic(t));
    let entry = || {
        let e = gtk::Entry::new();
        e.set_hexpand(true);
        e
    };
    let check = |t: &str| gtk::CheckButton::with_mnemonic(&gtk_mnemonic(t));
    let list = || {
        let l = gtk::ListBox::new();
        l.set_selection_mode(gtk::SelectionMode::Single);
        l.set_activate_on_single_click(false);
        l
    };
    let (label_row, label) = crate::add::entry_with_choices(&host.labels());
    let preview_rows: Rc<RefCell<Vec<[String; 2]>>> = Rc::default();
    let history_rows: Rc<RefCell<Vec<[String; 3]>>> = Rc::default();
    let rows = preview_rows.clone();
    let preview = Table::new(
        &[(S::ColName, 260, false), (S::ColFeed, 120, false)],
        Rc::new(move |r, c| rows.borrow().get(r).map(|row| row[c].clone()).unwrap_or_default()),
        None,
        None,
    );
    let rows = history_rows.clone();
    let history = Table::new(
        &view::HISTORY_COLUMNS,
        Rc::new(move |r, c| rows.borrow().get(r).map(|row| row[c].clone()).unwrap_or_default()),
        None,
        None,
    );
    let mut quality_checks = vec![gtk::CheckButton::with_label(tr!(RssQualityAll))];
    quality_checks.extend(Quality::LIST.iter().map(|(_, name)| gtk::CheckButton::with_label(name)));
    let interval = gtk::DropDown::from_strings(&MIN_INTERVALS.map(|(_, n)| lang::text(n)));
    let d = Rc::new(Dialog {
        host,
        window: window.downgrade(),
        feeds: list(),
        feed_rows: RefCell::default(),
        feed_status: gtk::Label::new(None),
        feed_edit: button(tr!(RssFeedsEdit)),
        feed_remove: button(tr!(RssFeedsRemove)),
        filters: list(),
        filter_rows: RefCell::default(),
        filter: Cell::new(None),
        name: entry(),
        pattern: entry(),
        not: entry(),
        save: entry(),
        save_browse: gtk::Button::with_label("..."),
        feed: gtk::DropDown::from_strings(&[]),
        feed_choices: RefCell::default(),
        quality: gtk::MenuButton::new(),
        quality_checks,
        episodes: entry(),
        interval,
        label,
        label_row,
        smart: check(tr!(RssSmart)),
        priority: check(tr!(RssHighPriority)),
        paused: check(tr!(RssPaused)),
        filter_remove: button(tr!(RssFilterRemove)),
        filter_up: button(tr!(RssFilterUp)),
        filter_down: button(tr!(RssFilterDown)),
        preview,
        preview_rows,
        history,
        history_rows,
        loading: Cell::new(false),
    });
    OPEN.with(|o| *o.borrow_mut() = Some(Rc::downgrade(&d)));

    // left: the feeds
    let feed_add = button(tr!(RssFeedsAdd));
    let feed_update = button(tr!(RssFeedsUpdate));
    let feeds_scroll = gtk::ScrolledWindow::builder().child(&d.feeds).vexpand(true).has_frame(true).build();
    d.feed_status.set_xalign(0.0);
    d.feed_status.set_wrap(true);
    d.feed_status.set_lines(3);
    d.feed_status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let feed_buttons = gtk::Grid::new();
    feed_buttons.set_row_spacing(4);
    feed_buttons.set_column_spacing(4);
    feed_buttons.set_column_homogeneous(true);
    feed_buttons.attach(&feed_add, 0, 0, 1, 1);
    feed_buttons.attach(&d.feed_edit, 1, 0, 1, 1);
    feed_buttons.attach(&d.feed_remove, 0, 1, 1, 1);
    feed_buttons.attach(&feed_update, 1, 1, 1, 1);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    left.set_size_request(220, -1);
    let feeds_title = gtk::Label::new(Some(&lang::plain(tr!(RssFeeds))));
    feeds_title.set_xalign(0.0);
    left.append(&feeds_title);
    left.append(&feeds_scroll);
    left.append(&d.feed_status);
    left.append(&feed_buttons);

    // the Filters tab: the list, its buttons, the fields and the preview
    let filters_scroll =
        gtk::ScrolledWindow::builder().child(&d.filters).vexpand(true).has_frame(true).build();
    let filter_add = button(tr!(RssFilterAdd));
    let filter_buttons = gtk::Grid::new();
    filter_buttons.set_row_spacing(4);
    filter_buttons.set_column_spacing(4);
    filter_buttons.set_column_homogeneous(true);
    filter_buttons.attach(&filter_add, 0, 0, 1, 1);
    filter_buttons.attach(&d.filter_remove, 1, 0, 1, 1);
    filter_buttons.attach(&d.filter_up, 0, 1, 1, 1);
    filter_buttons.attach(&d.filter_down, 1, 1, 1, 1);
    let filter_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    filter_list.set_size_request(180, -1);
    filter_list.append(&filters_scroll);
    filter_list.append(&filter_buttons);

    let quality_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    margins(&quality_box, 4);
    for c in &d.quality_checks {
        quality_box.append(c);
    }
    let popover = gtk::Popover::new();
    popover.set_child(Some(&quality_box));
    d.quality.set_popover(Some(&popover));
    d.quality.set_hexpand(true);
    let save_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    save_row.add_css_class("linked");
    save_row.append(&d.save);
    save_row.append(&d.save_browse);
    let episodes_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    d.episodes.set_width_chars(10);
    episodes_row.append(&d.episodes);
    let example = gtk::Label::new(Some(tr!(RssEpisodesExample)));
    example.add_css_class("dim-label");
    example.set_xalign(0.0);
    example.set_wrap(true);
    example.set_hexpand(true);
    episodes_row.append(&example);
    let fields = gtk::Grid::new();
    fields.set_row_spacing(6);
    fields.set_column_spacing(8);
    fields.set_hexpand(true);
    let rows: [(&str, gtk::Widget); 9] = [
        (tr!(RssName), d.name.clone().upcast()),
        (tr!(RssFilter), d.pattern.clone().upcast()),
        (tr!(RssNot), d.not.clone().upcast()),
        (tr!(RssSaveIn), save_row.upcast()),
        (tr!(RssFeed), d.feed.clone().upcast()),
        (tr!(RssQuality), d.quality.clone().upcast()),
        (tr!(RssEpisodes), episodes_row.upcast()),
        (tr!(RssMinInterval), d.interval.clone().upcast()),
        (tr!(RssLabel), d.label_row.clone().upcast()),
    ];
    for (i, (caption, widget)) in rows.into_iter().enumerate() {
        let l = gtk::Label::with_mnemonic(&gtk_mnemonic(caption));
        l.set_xalign(0.0);
        l.set_mnemonic_widget(Some(&widget));
        fields.attach(&l, 0, i as i32, 1, 1);
        widget.set_hexpand(true);
        fields.attach(&widget, 1, i as i32, 1, 1);
    }
    let checks = gtk::Box::new(gtk::Orientation::Vertical, 2);
    checks.append(&d.smart);
    checks.append(&d.priority);
    checks.append(&d.paused);
    let matching = gtk::Label::new(Some(&lang::plain(tr!(RssMatching))));
    matching.set_xalign(0.0);
    let preview_scroll = gtk::ScrolledWindow::builder()
        .child(&d.preview.view)
        .vexpand(true)
        .min_content_height(120)
        .has_frame(true)
        .build();
    let right = gtk::Box::new(gtk::Orientation::Vertical, 8);
    right.set_hexpand(true);
    right.append(&fields);
    right.append(&checks);
    right.append(&matching);
    right.append(&preview_scroll);
    let filters_page = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    margins(&filters_page, 8);
    filters_page.append(&filter_list);
    filters_page.append(&right);

    // the History tab
    let history_scroll =
        gtk::ScrolledWindow::builder().child(&d.history.view).vexpand(true).has_frame(true).build();
    let clear = button(tr!(RssClearHistory));
    clear.set_halign(gtk::Align::End);
    let history_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    margins(&history_page, 8);
    history_page.append(&history_scroll);
    history_page.append(&clear);

    let tabs = gtk::Notebook::new();
    tabs.set_hexpand(true);
    tabs.append_page(&filters_page, Some(&gtk::Label::new(Some(tr!(RssTabFilters)))));
    tabs.append_page(&history_page, Some(&gtk::Label::new(Some(tr!(RssTabHistory)))));
    let panes = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    panes.set_vexpand(true);
    panes.append(&left);
    panes.append(&tabs);
    let close = button(tr!(BtnClose));
    close.set_halign(gtk::Align::End);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    margins(&body, 10);
    body.append(&panes);
    body.append(&close);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&close));
    escape_closes(&window);

    // the Feeds list's right-click menu
    let group = gio::SimpleActionGroup::new();
    let act = |name: &str, f: fn(&Dialog)| {
        let a = gio::SimpleAction::new(name, None);
        let w = Rc::downgrade(&d);
        a.connect_activate(move |_, _| {
            if let Some(d) = w.upgrade() {
                f(&d);
            }
        });
        group.add_action(&a);
    };
    act("feed-add", Dialog::add_feed);
    act("feed-edit", Dialog::edit_feed);
    act("feed-toggle", Dialog::toggle_feed);
    act("feed-update", Dialog::update_feed);
    act("feed-remove", Dialog::remove_feed);
    window.insert_action_group("rss", Some(&group));
    let click = gtk::GestureClick::new();
    click.set_button(3);
    let w = Rc::downgrade(&d);
    click.connect_pressed(move |g, _, x, y| {
        let Some(d) = w.upgrade() else { return };
        if let Some(row) = d.feeds.row_at_y(y as i32) {
            d.feeds.select_row(Some(&row));
        }
        if let Some(widget) = g.widget() {
            d.feed_menu(&group, &widget, x, y);
        }
    });
    d.feeds.add_controller(click);

    // the handlers
    let on = |f: fn(&Dialog)| {
        let w = Rc::downgrade(&d);
        move || {
            if let Some(d) = w.upgrade() {
                f(&d);
            }
        }
    };
    let f = on(Dialog::add_feed);
    feed_add.connect_clicked(move |_| f());
    let f = on(Dialog::edit_feed);
    d.feed_edit.connect_clicked(move |_| f());
    let f = on(Dialog::remove_feed);
    d.feed_remove.connect_clicked(move |_| f());
    let f = on(Dialog::update_feed);
    feed_update.connect_clicked(move |_| f());
    let f = on(Dialog::add_filter);
    filter_add.connect_clicked(move |_| f());
    let f = on(Dialog::remove_filter);
    d.filter_remove.connect_clicked(move |_| f());
    let f = on(|d| d.move_filter(true));
    d.filter_up.connect_clicked(move |_| f());
    let f = on(|d| d.move_filter(false));
    d.filter_down.connect_clicked(move |_| f());
    let f = on(Dialog::pick_save_dir);
    d.save_browse.connect_clicked(move |_| f());
    let f = on(|d| {
        edit(&*d.host, |r| r.history.clear());
        d.fill_history();
    });
    clear.connect_clicked(move |_| f());
    let f = on(Dialog::feed_selected);
    d.feeds.connect_row_selected(move |_, _| f());
    let f = on(Dialog::edit_feed);
    d.feeds.connect_row_activated(move |_, _| f());
    let w = Rc::downgrade(&d);
    d.filters.connect_row_selected(move |_, row| {
        let (Some(d), Some(row)) = (w.upgrade(), row) else { return };
        if d.loading.get() {
            return;
        }
        let id = d.filter_rows.borrow().get(row.index() as usize).copied();
        if id.is_some() && id != d.filter.get() {
            d.show_filter(id);
        }
    });
    for (entry, preview) in [
        (&d.name, false),
        (&d.pattern, true),
        (&d.not, true),
        (&d.save, false),
        (&d.episodes, true),
        (&d.label, false),
    ] {
        let w = Rc::downgrade(&d);
        let is_name = entry == &d.name;
        entry.connect_changed(move |_| {
            if let Some(d) = w.upgrade() {
                d.store_fields(is_name, preview);
            }
        });
    }
    for (dropdown, preview) in [(&d.feed, true), (&d.interval, false)] {
        let w = Rc::downgrade(&d);
        dropdown.connect_selected_notify(move |_| {
            if let Some(d) = w.upgrade() {
                d.store_fields(false, preview);
            }
        });
    }
    for c in [&d.smart, &d.priority, &d.paused] {
        let w = Rc::downgrade(&d);
        c.connect_toggled(move |_| {
            if let Some(d) = w.upgrade() {
                d.store_fields(false, false);
            }
        });
    }
    for (i, c) in d.quality_checks.iter().enumerate() {
        let w = Rc::downgrade(&d);
        c.connect_toggled(move |_| {
            if let Some(d) = w.upgrade() {
                d.toggle_quality(i);
            }
        });
    }
    let w = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    window.connect_close_request({
        let d = d.clone();
        move |_| {
            let _ = &d;
            OPEN.with(|o| o.borrow_mut().take());
            closed();
            glib::Propagation::Proceed
        }
    });

    d.fill_feeds(feed);
    d.fill_filters(None);
    d.fill_history();
    window.present();
}

impl Dialog {
    fn parent(&self) -> Option<gtk::Window> {
        self.window.upgrade()
    }

    /// Everything again after the downloader changed underneath (a fetch finished).
    fn refresh(&self) {
        self.fill_feeds(self.selected_feed());
        self.fill_filters(self.filter.get());
        self.fill_history();
    }

    /// The feed picked in the Feeds list (`None`: All Feeds or nothing).
    fn selected_feed(&self) -> Option<FeedId> {
        let row = self.feeds.selected_row()?;
        self.feed_rows.borrow().get(row.index() as usize).copied().flatten()
    }

    /// The Feeds list (All Feeds, then each feed) with `select` picked, the Feed field's
    /// choices, and the state line.
    fn fill_feeds(&self, select: Option<FeedId>) {
        let Some(feeds) =
            read(&*self.host, |d| d.feeds.iter().map(|f| (f.id, f.name().to_owned())).collect::<Vec<_>>())
        else {
            return;
        };
        self.loading.set(true);
        while let Some(row) = self.feeds.row_at_index(0) {
            self.feeds.remove(&row);
        }
        let mut rows = vec![None];
        let mut names = vec![tr!(RssAllFeeds).to_owned()];
        for (id, name) in &feeds {
            rows.push(Some(*id));
            names.push(name.clone());
        }
        for name in &names {
            let l = gtk::Label::new(Some(name));
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            margins(&l, 4);
            self.feeds.append(&l);
        }
        let index = rows.iter().position(|f| *f == select).unwrap_or(0);
        *self.feed_rows.borrow_mut() = rows.clone();
        self.feeds.select_row(self.feeds.row_at_index(index as i32).as_ref());
        let current = self.feed_choices.borrow().get(self.feed.selected() as usize).copied().flatten();
        let strings: Vec<&str> = names.iter().map(String::as_str).collect();
        self.feed.set_model(Some(&gtk::StringList::new(&strings)));
        *self.feed_choices.borrow_mut() = rows;
        let at = self.feed_choices.borrow().iter().position(|c| *c == current).unwrap_or(0);
        self.feed.set_selected(at as u32);
        self.loading.set(false);
        self.feed_selected();
    }

    /// The state line and the buttons for the feed picked in the list.
    fn feed_selected(&self) {
        if self.loading.get() {
            return;
        }
        let feed = self.selected_feed();
        let text = match feed {
            Some(id) => {
                let updating = self.host.updating(id);
                read(&*self.host, |d| d.feed(id).map(|f| view::feed_status(f, updating)))
                    .flatten()
                    .unwrap_or_default()
            }
            None => {
                let n = read(&*self.host, |d| d.feeds.len()).unwrap_or(0);
                tr!(RssFeedCount, count = n)
            }
        };
        self.feed_status.set_text(&text);
        self.feed_edit.set_sensitive(feed.is_some());
        self.feed_remove.set_sensitive(feed.is_some());
    }

    /// The Feeds list's right-click menu: Add, Edit, Enable / Disable, Update, Remove.
    fn feed_menu(&self, group: &gio::SimpleActionGroup, widget: &gtk::Widget, x: f64, y: f64) {
        let feed = self.selected_feed();
        let enabled = feed.and_then(|id| read(&*self.host, |d| d.feed(id).map(|f| f.enabled)).flatten());
        for name in ["feed-edit", "feed-toggle", "feed-remove"] {
            if let Some(a) = group.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                a.set_enabled(feed.is_some());
            }
        }
        let menu = gio::Menu::new();
        let item = |text: &str, action: &str| menu.append(Some(&gtk_mnemonic(text)), Some(action));
        item(tr!(MenuAddFeed), "rss.feed-add");
        item(tr!(MenuEditFeed), "rss.feed-edit");
        item(
            if enabled == Some(false) { tr!(MenuEnableFeed) } else { tr!(MenuDisableFeed) },
            "rss.feed-toggle",
        );
        item(tr!(MenuUpdateFeed), "rss.feed-update");
        item(tr!(MenuDeleteFeed), "rss.feed-remove");
        crate::app::popup_menu(&menu, self.feeds.upcast_ref(), widget, x, y);
    }

    fn add_feed(&self) {
        let Some(parent) = self.parent() else { return };
        let (host, window) = (self.host.clone(), self.window.clone());
        add_feed_dialog(&parent, move |form| {
            let mut id = 0;
            edit(&*host, |d| id = form.add_to(d));
            host.update(Some(id));
            if let Some(d) = OPEN.with(|o| o.borrow().as_ref().and_then(Weak::upgrade)) {
                if d.window == window {
                    d.fill_feeds(Some(id));
                }
            }
        });
    }

    fn edit_feed(&self) {
        let Some(id) = self.selected_feed() else { return };
        let Some(parent) = self.parent() else { return };
        let Some(form) = read(&*self.host, |d| d.feed(id).map(FeedForm::of)).flatten() else { return };
        let host = self.host.clone();
        feed_dialog(&parent, tr!(RssEditFeed), form, move |form| {
            let mut moved = false;
            edit(&*host, |d| moved = form.store(d, id));
            if moved {
                host.update(Some(id));
            }
            changed();
        });
    }

    fn toggle_feed(&self) {
        let Some(id) = self.selected_feed() else { return };
        edit(&*self.host, |d| {
            if let Some(f) = d.feed_mut(id) {
                f.enabled = !f.enabled;
            }
        });
        self.feed_selected();
    }

    fn update_feed(&self) {
        self.host.update(self.selected_feed());
        self.feed_selected();
    }

    fn remove_feed(&self) {
        let Some(id) = self.selected_feed() else { return };
        let name =
            read(&*self.host, |d| d.feed(id).map(|f| f.name().to_owned())).flatten().unwrap_or_default();
        let d = gtk::AlertDialog::builder()
            .message(tr!(AskDeleteFeed, name))
            .buttons([tr!(BtnCancel), tr!(BtnOk)])
            .cancel_button(0)
            .default_button(1)
            .modal(true)
            .build();
        let host = self.host.clone();
        d.choose(self.parent().as_ref(), None::<&gio::Cancellable>, move |r| {
            if r != Ok(1) {
                return;
            }
            edit(&*host, |d| d.remove_feed(id));
            if let Some(d) = OPEN.with(|o| o.borrow().as_ref().and_then(Weak::upgrade)) {
                d.fill_feeds(None);
                d.fill_filters(d.filter.get());
            }
        });
    }

    /// The filter list with `select` picked (else the first filter), and its fields.
    fn fill_filters(&self, select: Option<FilterId>) {
        let Some(filters) = read(&*self.host, |d| {
            d.filters.iter().map(|f| (f.id, f.name.clone(), f.enabled)).collect::<Vec<_>>()
        }) else {
            return;
        };
        self.loading.set(true);
        while let Some(row) = self.filters.row_at_index(0) {
            self.filters.remove(&row);
        }
        let mut ids = Vec::new();
        for (id, name, on) in filters {
            let c = gtk::CheckButton::with_label(&name);
            c.set_active(on);
            margins(&c, 2);
            let host = self.host.clone();
            c.connect_toggled(move |c| {
                let on = c.is_active();
                edit(&*host, |d| {
                    if let Some(f) = d.filter_mut(id) {
                        f.enabled = on;
                    }
                });
            });
            self.filters.append(&c);
            ids.push(id);
        }
        let row = select.and_then(|s| ids.iter().position(|&f| f == s)).or((!ids.is_empty()).then_some(0));
        if let Some(r) = row {
            self.filters.select_row(self.filters.row_at_index(r as i32).as_ref());
        }
        let shown = row.map(|r| ids[r]);
        *self.filter_rows.borrow_mut() = ids;
        self.loading.set(false);
        self.show_filter(shown);
    }

    /// Writes filter `id`'s fields into the controls (empty and greyed without one).
    fn show_filter(&self, id: Option<FilterId>) {
        self.filter.set(id);
        let f = id.and_then(|id| read(&*self.host, |d| d.filter(id).cloned()).flatten());
        let f2 = f.clone().unwrap_or_else(|| RssFilter { name: String::new(), ..RssFilter::default() });
        self.loading.set(true);
        for (e, text) in [
            (&self.name, &f2.name),
            (&self.pattern, &f2.pattern),
            (&self.not, &f2.not),
            (&self.save, &f2.save_in),
            (&self.episodes, &f2.episodes),
            (&self.label, &f2.label),
        ] {
            e.set_text(text);
        }
        let feed = self.feed_choices.borrow().iter().position(|&c| c == f2.feed).unwrap_or(0);
        self.feed.set_selected(feed as u32);
        let interval = MIN_INTERVALS.iter().position(|&(s, _)| s == f2.min_interval);
        self.interval.set_selected(interval.map_or(gtk::INVALID_LIST_POSITION, |i| i as u32));
        self.show_quality(f2.quality);
        self.smart.set_active(f2.smart);
        self.priority.set_active(f2.top_priority);
        self.paused.set_active(f2.paused);
        let on = f.is_some();
        for w in [
            self.name.upcast_ref::<gtk::Widget>(),
            self.pattern.upcast_ref(),
            self.not.upcast_ref(),
            self.save.upcast_ref(),
            self.save_browse.upcast_ref(),
            self.feed.upcast_ref(),
            self.quality.upcast_ref(),
            self.episodes.upcast_ref(),
            self.interval.upcast_ref(),
            self.label_row.upcast_ref(),
            self.smart.upcast_ref(),
            self.priority.upcast_ref(),
            self.paused.upcast_ref(),
            self.filter_remove.upcast_ref(),
            self.filter_up.upcast_ref(),
            self.filter_down.upcast_ref(),
        ] {
            w.set_sensitive(on);
        }
        self.loading.set(false);
        self.fill_preview();
    }

    /// The Quality button's text and its menu's check marks.
    fn show_quality(&self, q: Quality) {
        let was = self.loading.replace(true);
        self.quality.set_label(&quality_text(q));
        if let Some((all, rest)) = self.quality_checks.split_first() {
            all.set_active(q.is_all());
            for (c, (one, _)) in rest.iter().zip(Quality::LIST.iter()) {
                c.set_active(q.contains(*one) && !q.is_all());
            }
        }
        self.loading.set(was);
    }

    /// The items of the feeds' last fetches the filter matches on its fields.
    fn fill_preview(&self) {
        let rows = self
            .filter
            .get()
            .and_then(|id| {
                read(&*self.host, |d| {
                    let Some(f) = d.filter(id) else { return Vec::new() };
                    d.matching_items(f)
                        .take(PREVIEW_ROWS)
                        .map(|(feed, i)| {
                            [i.title.clone(), d.feed(feed).map(|f| f.name().to_owned()).unwrap_or_default()]
                        })
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        let n = rows.len();
        *self.preview_rows.borrow_mut() = rows;
        self.preview.update(n);
    }

    fn fill_history(&self) {
        let rows = read(&*self.host, view::history_rows).unwrap_or_default();
        let n = rows.len();
        *self.history_rows.borrow_mut() = rows;
        self.history.update(n);
    }

    /// Stores the fields into the selected filter after an edit (`name`: the Name field, whose
    /// row follows; `preview`: a field the preview depends on).
    fn store_fields(&self, name: bool, preview: bool) {
        if self.loading.get() {
            return;
        }
        let Some(id) = self.filter.get() else { return };
        let feed = self.feed_choices.borrow().get(self.feed.selected() as usize).copied().flatten();
        let interval = MIN_INTERVALS.get(self.interval.selected() as usize).map(|&(s, _)| s);
        let text = |e: &gtk::Entry| e.text().to_string();
        let (n, pattern, not, save_in, episodes, label) = (
            text(&self.name),
            text(&self.pattern),
            text(&self.not),
            text(&self.save),
            text(&self.episodes),
            text(&self.label),
        );
        let (smart, top, paused) =
            (self.smart.is_active(), self.priority.is_active(), self.paused.is_active());
        let shown = n.clone();
        edit(&*self.host, |d| {
            let Some(f) = d.filter_mut(id) else { return };
            f.name = n;
            f.pattern = pattern;
            f.not = not;
            f.save_in = save_in.trim().to_owned();
            f.feed = feed;
            f.episodes = episodes;
            if let Some(s) = interval {
                f.min_interval = s;
            }
            f.label = label.trim().to_owned();
            f.smart = smart;
            f.top_priority = top;
            f.paused = paused;
        });
        if name {
            let row = self.filter_rows.borrow().iter().position(|&f| f == id);
            if let Some(c) = row
                .and_then(|r| self.filters.row_at_index(r as i32))
                .and_then(|r| r.child())
                .and_downcast::<gtk::CheckButton>()
            {
                c.set_label(Some(&shown));
            }
        }
        if preview {
            self.fill_preview();
        }
    }

    /// A Quality menu entry toggled: 0 is All, `1 + i` is `Quality::LIST[i]`.
    fn toggle_quality(&self, choice: usize) {
        if self.loading.get() {
            return;
        }
        let Some(id) = self.filter.get() else { return };
        let mut q = Quality::ALL;
        edit(&*self.host, |d| {
            let Some(f) = d.filter_mut(id) else { return };
            f.quality = match choice {
                0 => Quality::ALL,
                // All is no bit set: the first pick is that quality alone
                c => match Quality::LIST.get(c - 1) {
                    Some(&(one, _)) if f.quality.contains(one) => Quality(f.quality.0 & !one.0),
                    Some(&(one, _)) => f.quality.union(one),
                    None => f.quality,
                },
            };
            q = f.quality;
        });
        self.show_quality(q);
        self.fill_preview();
    }

    fn add_filter(&self) {
        // µTorrent bound a new filter to the feed picked on the left
        let feed = self.selected_feed();
        let mut id = 0;
        edit(&*self.host, |d| id = d.add_filter(RssFilter { feed, ..RssFilter::default() }));
        self.fill_filters(Some(id));
        self.name.grab_focus();
        self.name.select_region(0, -1);
    }

    fn remove_filter(&self) {
        let Some(filter) = self.filter.get() else { return };
        let rows = self.filter_rows.borrow().clone();
        let row = rows.iter().position(|&f| f == filter).unwrap_or(0);
        edit(&*self.host, |d| d.remove_filter(filter));
        // the one that took its place, else the one above
        let rest: Vec<FilterId> = rows.into_iter().filter(|&f| f != filter).collect();
        self.fill_filters(rest.get(row).or(rest.last()).copied());
    }

    fn move_filter(&self, up: bool) {
        let Some(filter) = self.filter.get() else { return };
        edit(&*self.host, |d| d.move_filter(filter, up));
        self.fill_filters(Some(filter));
    }

    fn pick_save_dir(&self) {
        let Some(parent) = self.parent() else { return };
        let dialog = gtk::FileDialog::builder().title(lang::plain(tr!(RssPickSaveDir))).modal(true).build();
        let now = self.save.text();
        if std::path::Path::new(now.trim()).is_dir() {
            dialog.set_initial_folder(Some(&gio::File::for_path(now.trim())));
        }
        let entry = self.save.downgrade();
        dialog.select_folder(Some(&parent), None::<&gio::Cancellable>, move |r| {
            if let (Some(path), Some(e)) = (r.ok().and_then(|f| f.path()), entry.upgrade()) {
                e.set_text(&path.to_string_lossy());
            }
        });
    }
}
