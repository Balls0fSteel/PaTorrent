//! The Torrent Properties dialog (µTorrent 2.2.1), as the Win32 client's: the General tab's
//! Trackers, Bandwidth Settings, Seeding Goal and Other Settings for the selected torrents (the
//! Advanced tab is not built).
//!
//! The fields come from [`patorrent_frontend::properties`]. With several torrents selected the
//! check boxes have three states (a click goes off, on, mixed, as a Win32 three-state box) and
//! fields whose values differ start blank or mixed; what the user leaves that way stays as it is
//! on every torrent. OK hands the [`Changes`] to the caller to apply to each torrent.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use patorrent_frontend::lang::{self, gtk_mnemonic};
use patorrent_frontend::properties::{Changes, Field, Fields, Properties, Tri};
use patorrent_frontend::tr;

/// A check box with the dialog's three states.
#[derive(Clone)]
struct TriCheck {
    button: gtk::CheckButton,
    state: Rc<Cell<Tri>>,
    /// Set while the box is written, so its own toggle is not taken for a click.
    writing: Rc<Cell<bool>>,
}

impl TriCheck {
    fn new(text: &str, three_state: bool) -> TriCheck {
        let button = gtk::CheckButton::with_mnemonic(&gtk_mnemonic(text));
        let c = TriCheck { button, state: Rc::new(Cell::new(Tri::Off)), writing: Rc::new(Cell::new(false)) };
        let (state, writing) = (c.state.clone(), c.writing.clone());
        // the button is the signal's argument: holding it here would keep it alive for good
        c.button.connect_toggled(move |button| {
            if writing.get() {
                return;
            }
            let next = match state.get() {
                Tri::Off => Tri::On,
                Tri::On if three_state => Tri::Mixed,
                Tri::On | Tri::Mixed => Tri::Off,
            };
            write(button, &state, &writing, next);
        });
        c
    }

    fn get(&self) -> Tri {
        self.state.get()
    }

    fn set(&self, v: Tri) {
        write(&self.button, &self.state, &self.writing, v);
    }
}

fn write(button: &gtk::CheckButton, state: &Cell<Tri>, writing: &Cell<bool>, v: Tri) {
    writing.set(true);
    state.set(v);
    button.set_active(v == Tri::On);
    button.set_inconsistent(v == Tri::Mixed);
    writing.set(false);
}

/// What OK or Cancel hands back.
type Done = Box<dyn FnOnce(Option<Changes>)>;

struct Dialog {
    props: RefCell<Properties>,
    trackers: gtk::TextView,
    upload: gtk::Entry,
    download: gtk::Entry,
    override_slots: TriCheck,
    slots: gtk::Entry,
    override_goal: TriCheck,
    ratio: gtk::Entry,
    minutes: gtk::Entry,
    initial_seeding: TriCheck,
    dht: TriCheck,
    pex: TriCheck,
    done: RefCell<Option<Done>>,
}

/// Shows the dialog over `parent`, titled `title` ([`title`]); `done` gets the changes on OK
/// (possibly none) and `None` on Cancel.
pub fn show(
    parent: &gtk::ApplicationWindow,
    title: &str,
    props: Properties,
    done: impl FnOnce(Option<Changes>) + 'static,
) {
    let window = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(true)
        .default_width(520)
        .resizable(true)
        .build();
    let multi = props.count > 1;
    let number = || {
        let e = gtk::Entry::new();
        e.set_width_chars(8);
        e.set_max_width_chars(10);
        e.set_input_purpose(gtk::InputPurpose::Number);
        e.set_activates_default(true);
        e.set_halign(gtk::Align::End);
        e
    };
    let trackers = gtk::TextView::new();
    trackers.set_wrap_mode(gtk::WrapMode::None);
    trackers.set_accepts_tab(false);
    trackers.set_left_margin(4);
    trackers.set_right_margin(4);
    let d = Rc::new(Dialog {
        trackers,
        upload: number(),
        download: number(),
        override_slots: TriCheck::new(tr!(PropsOverrideSlots), multi),
        slots: number(),
        override_goal: TriCheck::new(tr!(PropsOverrideGoal), multi),
        ratio: number(),
        minutes: number(),
        initial_seeding: TriCheck::new(tr!(PropsInitialSeeding), multi),
        dht: TriCheck::new(tr!(PropsDht), multi),
        pex: TriCheck::new(tr!(PropsPex), multi),
        props: RefCell::new(props),
        done: RefCell::new(Some(Box::new(done))),
    });

    let tracker_scroll = gtk::ScrolledWindow::builder()
        .child(&d.trackers)
        .min_content_height(110)
        .vexpand(true)
        .has_frame(true)
        .build();
    let bandwidth = grid();
    labelled(&bandwidth, 0, tr!(PropsUpRate), &d.upload);
    labelled(&bandwidth, 1, tr!(PropsDownRate), &d.download);
    labelled(&bandwidth, 2, tr!(PropsSlots), &d.slots);
    bandwidth.attach(&d.override_slots.button, 0, 3, 2, 1);
    let goal = grid();
    goal.attach(&d.override_goal.button, 0, 0, 2, 1);
    labelled(&goal, 1, tr!(PropsRatio), &d.ratio);
    labelled(&goal, 2, tr!(PropsSeedTime), &d.minutes);
    let other = grid();
    other.set_column_homogeneous(true);
    other.attach(&d.initial_seeding.button, 0, 0, 1, 1);
    other.attach(&d.pex.button, 1, 0, 1, 1);
    other.attach(&d.dht.button, 0, 1, 1, 1);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
    page.set_margin_top(10);
    page.set_margin_bottom(10);
    page.set_margin_start(10);
    page.set_margin_end(10);
    for (title, child) in [
        (tr!(PropsTrackers), tracker_scroll.upcast_ref::<gtk::Widget>()),
        (tr!(PropsBandwidth), bandwidth.upcast_ref()),
        (tr!(PropsSeedingGoal), goal.upcast_ref()),
        (tr!(PropsOther), other.upcast_ref()),
    ] {
        let frame = gtk::Frame::new(Some(&lang::plain(title)));
        child.set_margin_top(4);
        child.set_margin_bottom(6);
        child.set_margin_start(8);
        child.set_margin_end(8);
        frame.set_child(Some(child));
        page.append(&frame);
    }
    let notebook = gtk::Notebook::new();
    notebook.append_page(&page, Some(&gtk::Label::new(Some(tr!(TabGeneral)))));
    notebook.set_vexpand(true);

    let ok = gtk::Button::with_label(tr!(BtnOk));
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label(tr!(BtnCancel));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&ok);
    buttons.append(&cancel);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(10);
    body.set_margin_end(10);
    body.append(&notebook);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&ok));
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);

    d.show_fields();
    d.initial_seeding.button.set_sensitive(d.props.borrow().can_initial_seed);
    d.update_sensitivity();
    for c in [&d.override_slots, &d.override_goal] {
        let w = Rc::downgrade(&d);
        c.button.connect_toggled(move |_| {
            if let Some(d) = w.upgrade() {
                d.update_sensitivity();
            }
        });
    }
    ok.connect_clicked({
        let (w, window) = (Rc::downgrade(&d), window.downgrade());
        move |_| {
            let (Some(d), Some(window)) = (w.upgrade(), window.upgrade()) else { return };
            d.props.borrow_mut().fields = d.read_fields();
            let result = d.props.borrow().changes();
            match result {
                Ok(c) => {
                    if let Some(done) = d.done.borrow_mut().take() {
                        done(Some(c));
                    }
                    window.close();
                }
                Err(e) => {
                    gtk::AlertDialog::builder().message(&e.message).modal(true).build().show(Some(&window));
                    d.focus(e.field);
                }
            }
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
    window.connect_close_request({
        let d = d.clone();
        move |_| {
            // Cancel, Escape and the title bar's close
            if let Some(done) = d.done.borrow_mut().take() {
                done(None);
            }
            glib::Propagation::Proceed
        }
    });
    window.present();
}

/// "Properties - name" for one torrent, the number of torrents for several.
pub fn title(names: &[String]) -> String {
    match names {
        [one] => tr!(PropsTitle, name = one),
        _ => tr!(PropsTitleMany, count = names.len()),
    }
}

fn grid() -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(12);
    g
}

/// A caption and its number on row `row`.
fn labelled(grid: &gtk::Grid, row: i32, text: &str, entry: &gtk::Entry) {
    let l = gtk::Label::with_mnemonic(&gtk_mnemonic(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_hexpand(true);
    l.set_mnemonic_widget(Some(entry));
    grid.attach(&l, 0, row, 1, 1);
    grid.attach(entry, 1, row, 1, 1);
}

impl Dialog {
    fn show_fields(&self) {
        let props = self.props.borrow();
        let f = &props.fields;
        self.trackers.buffer().set_text(&f.trackers);
        for (entry, text) in [
            (&self.upload, &f.upload_kib),
            (&self.download, &f.download_kib),
            (&self.slots, &f.upload_slots),
            (&self.ratio, &f.goal_ratio),
            (&self.minutes, &f.goal_minutes),
        ] {
            entry.set_text(text);
        }
        for (check, v) in [
            (&self.override_slots, f.override_slots),
            (&self.override_goal, f.override_goal),
            (&self.initial_seeding, f.initial_seeding),
            (&self.dht, f.dht),
            (&self.pex, f.pex),
        ] {
            check.set(v);
        }
    }

    fn read_fields(&self) -> Fields {
        let buffer = self.trackers.buffer();
        let (start, end) = buffer.bounds();
        Fields {
            trackers: buffer.text(&start, &end, false).to_string(),
            upload_kib: self.upload.text().to_string(),
            download_kib: self.download.text().to_string(),
            override_slots: self.override_slots.get(),
            upload_slots: self.slots.text().to_string(),
            override_goal: self.override_goal.get(),
            goal_ratio: self.ratio.text().to_string(),
            goal_minutes: self.minutes.text().to_string(),
            initial_seeding: self.initial_seeding.get(),
            dht: self.dht.get(),
            pex: self.pex.get(),
        }
    }

    /// The slot count and the goal's values are edited only while their override is not off.
    fn update_sensitivity(&self) {
        self.slots.set_sensitive(self.override_slots.get() != Tri::Off);
        let goal = self.override_goal.get() != Tri::Off;
        self.ratio.set_sensitive(goal);
        self.minutes.set_sensitive(goal);
    }

    /// Puts the cursor in the field a validation error is about, its text selected.
    fn focus(&self, field: Field) {
        let entry = match field {
            Field::Trackers => {
                self.trackers.grab_focus();
                return;
            }
            Field::UploadRate => &self.upload,
            Field::DownloadRate => &self.download,
            Field::UploadSlots => &self.slots,
            Field::GoalRatio => &self.ratio,
            Field::GoalMinutes => &self.minutes,
        };
        entry.grab_focus();
        entry.select_region(0, -1);
    }
}
