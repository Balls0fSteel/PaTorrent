//! File > Create New Torrent: µTorrent 2.2.1's dialog, as the Win32 client's. The files are
//! listed and hashed on a worker thread; the dialog follows its progress from a timer, and
//! Cancel (or closing the window) stops it through the progress callback.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_frontend::create::{self, CreateOptions, Created, FORMATS, PIECE_SIZES, Remembered};
use patorrent_frontend::lang::{self, gtk_mnemonic};
use patorrent_frontend::tr;

/// What the worker thread reports.
#[derive(Default)]
struct Progress {
    done: u32,
    total: u32,
    result: Option<Result<Created, String>>,
}

/// What the caller gets when the dialog closes: the fields to remember, and the torrent it made
/// with the folder to seed it from when "Start seeding" was on.
pub type Finished = Box<dyn FnOnce(Remembered, Option<(Created, Option<PathBuf>)>)>;

struct Dialog {
    window: glib::WeakRef<gtk::Window>,
    source: gtk::Entry,
    add_file: gtk::Button,
    add_dir: gtk::Button,
    skip: gtk::Entry,
    trackers: gtk::TextView,
    web_seeds: gtk::TextView,
    comment: gtk::Entry,
    piece: gtk::DropDown,
    format: gtk::DropDown,
    seed: gtk::CheckButton,
    private: gtk::CheckButton,
    order: gtk::CheckButton,
    progress: gtk::ProgressBar,
    create: gtk::Button,
    close: gtk::Button,
    remembered: RefCell<Remembered>,
    cancel: Arc<AtomicBool>,
    busy: Cell<bool>,
    /// Closed while hashing: close once the worker has stopped.
    closing: Cell<bool>,
    seed_from: RefCell<Option<PathBuf>>,
    created: RefCell<Option<Created>>,
    finished: RefCell<Option<Finished>>,
}

/// Shows the dialog over `parent`, its fields from `remembered`.
pub fn show(
    parent: &gtk::ApplicationWindow,
    remembered: Remembered,
    finished: impl FnOnce(Remembered, Option<(Created, Option<PathBuf>)>) + 'static,
) {
    let window = gtk::Window::builder()
        .title(tr!(CreateTitle))
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .build();
    let text_box = |lines: i32| {
        let v = gtk::TextView::new();
        v.set_wrap_mode(gtk::WrapMode::None);
        v.set_accepts_tab(false);
        v.set_left_margin(4);
        v.set_right_margin(4);
        let s =
            gtk::ScrolledWindow::builder().child(&v).min_content_height(lines * 18).has_frame(true).build();
        s.set_hexpand(true);
        (v, s)
    };
    let (trackers, trackers_scroll) = text_box(5);
    let (web_seeds, web_seeds_scroll) = text_box(2);
    let pieces: Vec<String> = PIECE_SIZES.iter().map(|&b| create::piece_size_text(b)).collect();
    let piece = gtk::DropDown::from_strings(&pieces.iter().map(String::as_str).collect::<Vec<_>>());
    let format = gtk::DropDown::from_strings(&FORMATS.map(|(t, _)| lang::text(t)));
    let entry = || {
        let e = gtk::Entry::new();
        e.set_hexpand(true);
        e
    };
    let check = |t: &str| gtk::CheckButton::with_mnemonic(&gtk_mnemonic(t));
    let d = Rc::new(Dialog {
        window: window.downgrade(),
        source: entry(),
        add_file: gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(CreateAddFile))),
        add_dir: gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(CreateAddDir))),
        skip: entry(),
        trackers,
        web_seeds,
        comment: entry(),
        piece,
        format,
        seed: check(tr!(CreateSeed)),
        private: check(tr!(CreatePrivate)),
        order: check(tr!(CreatePreserveOrder)),
        progress: gtk::ProgressBar::new(),
        create: gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(CreateSave))),
        close: gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnClose))),
        remembered: RefCell::new(remembered),
        cancel: Arc::new(AtomicBool::new(false)),
        busy: Cell::new(false),
        closing: Cell::new(false),
        seed_from: RefCell::default(),
        created: RefCell::default(),
        finished: RefCell::new(Some(Box::new(finished))),
    });

    // the layout: the Win32 dialog's groups, top to bottom
    let source_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    source_buttons.set_halign(gtk::Align::End);
    source_buttons.append(&d.add_file);
    source_buttons.append(&d.add_dir);
    let source = gtk::Box::new(gtk::Orientation::Vertical, 6);
    source.append(&d.source);
    source.append(&source_buttons);
    let props = gtk::Grid::new();
    props.set_row_spacing(6);
    props.set_column_spacing(8);
    let caption = |t: &str, mnemonic: &gtk::Widget| {
        let l = gtk::Label::with_mnemonic(&gtk_mnemonic(t));
        l.set_xalign(1.0);
        l.set_yalign(0.0);
        l.set_margin_top(4);
        l.set_mnemonic_widget(Some(mnemonic));
        l
    };
    props.attach(&caption(tr!(CreateTrackers), d.trackers.upcast_ref()), 0, 0, 1, 1);
    props.attach(&trackers_scroll, 1, 0, 3, 1);
    props.attach(&caption(tr!(CreateWebSeeds), d.web_seeds.upcast_ref()), 0, 1, 1, 1);
    props.attach(&web_seeds_scroll, 1, 1, 3, 1);
    props.attach(&caption(tr!(CreateComment), d.comment.upcast_ref()), 0, 2, 1, 1);
    props.attach(&d.comment, 1, 2, 3, 1);
    props.attach(&caption(tr!(CreatePieceSize), d.piece.upcast_ref()), 0, 3, 1, 1);
    props.attach(&d.piece, 1, 3, 1, 1);
    // BEP 52 (not in µTorrent 2.2.1): v1, hybrid or v2
    props.attach(&caption(tr!(CreateVersion), d.format.upcast_ref()), 2, 3, 1, 1);
    props.attach(&d.format, 3, 3, 1, 1);
    let other = gtk::Grid::new();
    other.set_row_spacing(4);
    other.set_column_spacing(12);
    other.set_column_homogeneous(true);
    other.attach(&d.seed, 0, 0, 1, 1);
    other.attach(&d.private, 1, 0, 1, 1);
    other.attach(&d.order, 0, 1, 2, 1);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(10);
    body.set_margin_end(10);
    for (title, child) in [
        (tr!(CreateSource), source.upcast_ref::<gtk::Widget>()),
        (tr!(CreateSkip), d.skip.upcast_ref()),
        (tr!(CreateProps), props.upcast_ref()),
        (tr!(CreateOther), other.upcast_ref()),
    ] {
        let frame = gtk::Frame::new(Some(&lang::plain(title)));
        child.set_margin_top(4);
        child.set_margin_bottom(6);
        child.set_margin_start(8);
        child.set_margin_end(8);
        frame.set_child(Some(child));
        body.append(&frame);
    }
    d.progress.set_show_text(true);
    d.progress.set_text(Some(""));
    body.append(&d.progress);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    d.create.add_css_class("suggested-action");
    buttons.append(&d.create);
    buttons.append(&d.close);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&d.create));
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);

    d.init();
    let w = Rc::downgrade(&d);
    d.format.connect_selected_notify(move |_| {
        if let Some(d) = w.upgrade() {
            d.sync_order();
        }
    });
    for folder in [false, true] {
        let w = Rc::downgrade(&d);
        let button = if folder { &d.add_dir } else { &d.add_file };
        button.connect_clicked(move |_| {
            if let Some(d) = w.upgrade() {
                d.pick_source(folder);
            }
        });
    }
    let w = Rc::downgrade(&d);
    d.create.connect_clicked(move |_| {
        if let Some(d) = w.upgrade() {
            d.start();
        }
    });
    let w = Rc::downgrade(&d);
    d.close.connect_clicked(move |_| {
        if let Some(window) = w.upgrade().and_then(|d| d.window.upgrade()) {
            window.close();
        }
    });
    window.connect_close_request({
        let d = d.clone();
        move |_| {
            if d.busy.get() {
                // the worker stops at its next piece, then the window closes
                d.cancel.store(true, Ordering::Relaxed);
                d.closing.set(true);
                return glib::Propagation::Stop;
            }
            if let Some(f) = d.finished.borrow_mut().take() {
                let created = d.created.borrow_mut().take().map(|c| (c, d.seed_from.borrow_mut().take()));
                f(d.remembered.borrow().clone(), created);
            }
            glib::Propagation::Proceed
        }
    });
    window.present();
}

fn text_of(v: &gtk::TextView) -> String {
    let b = v.buffer();
    let (start, end) = b.bounds();
    b.text(&start, &end, false).to_string()
}

impl Dialog {
    fn init(&self) {
        let r = self.remembered.borrow();
        self.trackers.buffer().set_text(&r.trackers.replace("\r\n", "\n"));
        self.piece.set_selected(r.piece.min(PIECE_SIZES.len() - 1) as u32);
        self.format.set_selected(r.format.index() as u32);
        self.seed.set_active(r.seed);
        self.private.set_active(r.private);
        self.order.set_active(r.preserve_order);
        drop(r);
        self.sync_order();
    }

    fn chosen_format(&self) -> create::TorrentFormat {
        FORMATS[(self.format.selected() as usize).min(FORMATS.len() - 1)].1
    }

    /// "Preserve file order" applies to v1 only (v2 lays the files out sorted).
    /// "Preserve file order" applies to v1 only (v2 lays the files out sorted).
    fn sync_order(&self) {
        self.order.set_sensitive(!self.busy.get() && self.chosen_format().allows_any_order());
        // a remote server has not got the files: the torrent cannot be seeded from here
        if crate::remote::is_remote() {
            self.seed.set_active(false);
            self.seed.set_sensitive(false);
        }
    }

    fn error(&self, message: &str) {
        let window = self.window.upgrade();
        gtk::AlertDialog::builder().message(message).modal(true).build().show(window.as_ref());
    }

    /// "Add file" and "Add directory".
    fn pick_source(&self, folder: bool) {
        let Some(window) = self.window.upgrade() else { return };
        let title = if folder { tr!(CreatePickDir) } else { tr!(CreateAddFile) };
        let dialog = gtk::FileDialog::builder().title(lang::plain(title)).modal(true).build();
        let now = self.source.text();
        let now = Path::new(now.trim());
        if now.is_dir() {
            dialog.set_initial_folder(Some(&gio::File::for_path(now)));
        } else if now.is_file() {
            dialog.set_initial_file(Some(&gio::File::for_path(now)));
        }
        let entry = self.source.downgrade();
        let done = move |r: Result<gio::File, glib::Error>| {
            if let (Some(path), Some(entry)) = (r.ok().and_then(|f| f.path()), entry.upgrade()) {
                entry.set_text(&path.to_string_lossy());
            }
        };
        if folder {
            dialog.select_folder(Some(&window), None::<&gio::Cancellable>, done);
        } else {
            dialog.open(Some(&window), None::<&gio::Cancellable>, done);
        }
    }

    /// "Create and save as...": checks the source, asks where to save, then hashes.
    fn start(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let Some((opts, seed_from)) = self.collect() else { return };
        let Some(window) = self.window.upgrade() else { return };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(tr!(FilterTorrents)));
        filter.add_pattern("*.torrent");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(lang::plain(tr!(CreatePickSave)))
            .modal(true)
            .filters(&filters)
            .initial_name(create::default_file_name(&opts.source))
            .build();
        if let Ok(dir) = create::seed_save_path(&opts.source) {
            dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
        }
        let w = Rc::downgrade(self);
        // the file chooser asks before replacing a file
        dialog.save(Some(&window), None::<&gio::Cancellable>, move |r| {
            let (Some(d), Some(file)) = (w.upgrade(), r.ok().and_then(|f| f.path())) else { return };
            d.hash(opts, seed_from, file);
        });
    }

    /// Reads and checks the fields (and remembers them); `None` after telling the user what is
    /// wrong.
    fn collect(&self) -> Option<(CreateOptions, Option<PathBuf>)> {
        let source = PathBuf::from(self.source.text().trim().trim_matches('"'));
        if source.as_os_str().is_empty() {
            self.error(tr!(CreateNoSource));
            self.source.grab_focus();
            return None;
        }
        if let Err(e) = std::fs::metadata(&source) {
            self.error(&format!("{}:\n{e}", source.display()));
            self.source.grab_focus();
            return None;
        }
        let piece = (self.piece.selected() as usize).min(PIECE_SIZES.len() - 1);
        let opts = CreateOptions {
            source: source.clone(),
            skip: self.skip.text().to_string(),
            trackers: text_of(&self.trackers),
            web_seeds: text_of(&self.web_seeds),
            comment: self.comment.text().to_string(),
            piece_size: PIECE_SIZES[piece],
            private: self.private.is_active(),
            preserve_order: self.order.is_active(),
            format: self.chosen_format(),
        };
        let seed = self.seed.is_active();
        *self.remembered.borrow_mut() = Remembered {
            trackers: opts.trackers.clone(),
            piece,
            seed,
            private: opts.private,
            preserve_order: opts.preserve_order,
            format: opts.format,
        };
        let seed_from = if seed {
            match create::seed_save_path(&source) {
                Ok(p) => Some(p),
                Err(e) => {
                    self.error(&format!("{}:\n{e}", source.display()));
                    return None;
                }
            }
        } else {
            None
        };
        Some((opts, seed_from))
    }

    /// Hashes on a worker thread and writes `file`; the timer shows the progress and finishes.
    fn hash(self: &Rc<Self>, opts: CreateOptions, seed_from: Option<PathBuf>, file: PathBuf) {
        *self.seed_from.borrow_mut() = seed_from;
        self.cancel.store(false, Ordering::Relaxed);
        self.set_busy(true);
        let shared = Arc::new(Mutex::new(Progress::default()));
        let (cancel, report) = (self.cancel.clone(), shared.clone());
        std::thread::spawn(move || {
            let result = create::build(&opts, &mut |done, total| {
                if let Ok(mut p) = report.lock() {
                    (p.done, p.total) = (done, total);
                }
                !cancel.load(Ordering::Relaxed)
            })
            .and_then(|c| match std::fs::write(&file, &c.bytes) {
                Ok(()) => Ok(c),
                Err(e) => Err(format!("{}: {e}", file.display())),
            });
            if let Ok(mut p) = report.lock() {
                p.result = Some(result);
            }
        });
        let w = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(d) = w.upgrade() else { return glib::ControlFlow::Break };
            let (done, total, result) = match shared.lock() {
                Ok(mut p) => (p.done, p.total, p.result.take()),
                Err(_) => (0, 0, Some(Err("the worker stopped".to_owned()))),
            };
            let fraction = f64::from(done) / f64::from(total.max(1));
            d.progress.set_fraction(fraction);
            d.progress.set_text(Some(&format!("{:.1} %", fraction * 100.0)));
            match result {
                Some(r) => {
                    d.finished(r);
                    glib::ControlFlow::Break
                }
                None => glib::ControlFlow::Continue,
            }
        });
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        for w in [
            self.source.upcast_ref::<gtk::Widget>(),
            self.add_file.upcast_ref(),
            self.add_dir.upcast_ref(),
            self.skip.upcast_ref(),
            self.trackers.upcast_ref(),
            self.web_seeds.upcast_ref(),
            self.comment.upcast_ref(),
            self.piece.upcast_ref(),
            self.format.upcast_ref(),
            self.seed.upcast_ref(),
            self.private.upcast_ref(),
            self.create.upcast_ref(),
        ] {
            w.set_sensitive(!busy);
        }
        self.sync_order();
        self.close.set_label(&gtk_mnemonic(if busy { tr!(BtnCancel) } else { tr!(BtnClose) }));
        self.close.set_use_underline(true);
        if busy {
            self.progress.set_fraction(0.0);
            self.progress.set_text(Some("0.0 %"));
            self.close.grab_focus();
        }
    }

    fn finished(&self, result: Result<Created, String>) {
        self.set_busy(false);
        match result {
            Ok(created) => {
                *self.created.borrow_mut() = Some(created);
                if let Some(w) = self.window.upgrade() {
                    w.close();
                }
            }
            Err(e) => {
                if self.closing.get() {
                    if let Some(w) = self.window.upgrade() {
                        w.close();
                    }
                    return;
                }
                self.progress.set_fraction(0.0);
                self.progress.set_text(Some(""));
                if e != "cancelled" {
                    self.error(&tr!(CreateFailed, error = e));
                }
                self.create.grab_focus();
            }
        }
    }
}
