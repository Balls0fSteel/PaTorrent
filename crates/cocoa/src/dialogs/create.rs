//! Create New Torrent (µTorrent 2.2.1's dialog, as the GTK and Win32 ones): the source file or
//! folder, files to skip, trackers, web seeds, comment, piece size, BitTorrent version, Start
//! seeding, Private and Preserve file order. "Create and save as…" asks where to save, then the
//! files are hashed on a worker thread; its progress comes back through `mainq::on_main`, and
//! Cancel (the Close button while it works, or closing the window) stops it at the next piece.
//! Closing the dialog remembers its fields and, after a torrent was made with "Start seeding",
//! adds it to seed from the source's folder (`Core::created`).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSButton, NSPopUpButton, NSProgressIndicator, NSProgressIndicatorStyle, NSTextField, NSTextView, NSView,
    NSWindow,
};
use rookparcel_frontend::create::{
    self, CreateOptions, Created, FORMATS, PIECE_SIZES, Remembered, TorrentFormat,
};
use rookparcel_frontend::{lang, tr};

use super::properties::text_box;
use super::{NOTHING, Observer, cmd};
use crate::app::{apply, peek, with_core};
use crate::ui::{self, layout, ns};

const ADD_FILE: u8 = 1;
const ADD_DIR: u8 = 2;
const CREATE: u8 = 3;
/// The Close button (Cancel while hashing).
const CLOSE: u8 = 4;
/// The window closed.
const CLOSED: u8 = 5;
/// The version changed: "Preserve file order" follows.
const FORMAT: u8 = 6;
/// The worker reported.
const PROGRESS: u8 = 7;
/// `ROOKPARCEL_TEST_ACCEPT`: the source from `ROOKPARCEL_TEST_PICK`, then Create.
const TEST: u8 = 8;

/// What the worker thread reports.
#[derive(Default)]
struct Progress {
    done: u32,
    total: u32,
    result: Option<Result<Created, String>>,
}

struct Dialog {
    id: u16,
    window: Retained<NSWindow>,
    source: Retained<NSTextField>,
    add_file: Retained<NSButton>,
    add_dir: Retained<NSButton>,
    skip: Retained<NSTextField>,
    trackers: Retained<NSTextView>,
    web_seeds: Retained<NSTextView>,
    comment: Retained<NSTextField>,
    piece: Retained<NSPopUpButton>,
    format: Retained<NSPopUpButton>,
    seed: Retained<NSButton>,
    private: Retained<NSButton>,
    order: Retained<NSButton>,
    progress: Retained<NSProgressIndicator>,
    percent: Retained<NSTextField>,
    create: Retained<NSButton>,
    close: Retained<NSButton>,
    remote: bool,
    remembered: RefCell<Remembered>,
    cancel: Arc<AtomicBool>,
    shared: Arc<Mutex<Progress>>,
    busy: Cell<bool>,
    /// The window closed while hashing: finish once the worker has stopped.
    closing: Cell<bool>,
    /// The torrent being written and where it is seeded from ("Start seeding").
    file: RefCell<Option<PathBuf>>,
    seed_from: RefCell<Option<PathBuf>>,
    created: RefCell<Option<Created>>,
    observer: RefCell<Option<Observer>>,
}

/// File > Create New Torrent.
pub fn open() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some((remembered, remote)) = peek(|c| (Remembered::load(c.prefs()), c.is_remote())) else { return };
    show(remembered, remote, mtm);
}

fn show(r: Remembered, remote: bool, mtm: MainThreadMarker) {
    let id = super::new_id();
    let source = ui::field("", "", mtm);
    layout::stretch(&source);
    let add_file = ui::button_tr(tr!(CreateAddFile), cmd(id, ADD_FILE), mtm);
    let add_dir = ui::button_tr(tr!(CreateAddDir), cmd(id, ADD_DIR), mtm);
    let source_buttons = layout::hstack(&[&layout::spacer(mtm), &add_file, &add_dir], mtm);
    let source_part = super::column(&[&source, &source_buttons], mtm);
    let skip = ui::field("", "", mtm);

    let (tracker_scroll, trackers) = text_box(&r.trackers.replace("\r\n", "\n"), 90.0, mtm);
    let (seed_scroll, web_seeds) = text_box("", 40.0, mtm);
    let comment = ui::field("", "", mtm);
    let pieces: Vec<String> = PIECE_SIZES.iter().map(|&b| create::piece_size_text(b)).collect();
    let piece = ui::popup(&pieces, r.piece.min(PIECE_SIZES.len() - 1), cmd(id, NOTHING), mtm);
    let formats: Vec<String> = FORMATS.iter().map(|(t, _)| ui::text(lang::text(*t))).collect();
    // BEP 52 (not in µTorrent 2.2.1): hybrid, v1 or v2
    let format = ui::popup(&formats, r.format.index(), cmd(id, FORMAT), mtm);
    let caption = |s: &str| ui::label_tr(s, mtm);
    let (c_trackers, c_seeds, c_comment, c_piece, c_version) = (
        caption(tr!(CreateTrackers)),
        caption(tr!(CreateWebSeeds)),
        caption(tr!(CreateComment)),
        caption(tr!(CreatePieceSize)),
        caption(tr!(CreateVersion)),
    );
    let piece_row = layout::hstack(&[&piece, &c_version, &format, &layout::spacer(mtm)], mtm);
    let props = layout::grid(
        &[
            vec![Some(&*c_trackers), Some(&*tracker_scroll)],
            vec![Some(&*c_seeds), Some(&*seed_scroll)],
            vec![Some(&*c_comment), Some(&*comment)],
            vec![Some(&*c_piece), Some(&*piece_row)],
        ],
        mtm,
    );
    layout::label_column(&props);
    layout::min_width(&tracker_scroll, 400.0);

    let check = |s: &str, on: bool| ui::check(&ui::text(s), on, cmd(id, NOTHING), mtm);
    let seed = check(tr!(CreateSeed), r.seed);
    let private = check(tr!(CreatePrivate), r.private);
    let order = check(tr!(CreatePreserveOrder), r.preserve_order);
    let other = layout::grid(&[vec![Some(&*seed), Some(&*private)], vec![Some(&*order), None]], mtm);
    other.setColumnSpacing(24.0);

    let progress = NSProgressIndicator::new(mtm);
    progress.setStyle(NSProgressIndicatorStyle::Bar);
    progress.setIndeterminate(false);
    progress.setMinValue(0.0);
    progress.setMaxValue(1.0);
    ui::auto(&progress);
    layout::stretch(&progress);
    let percent = ui::label("", mtm);
    layout::min_width(&percent, 60.0);
    let progress_row = layout::hstack(&[&progress, &percent], mtm);
    progress_row.setDistribution(objc2_app_kit::NSStackViewDistribution::Fill);

    let create = ui::button_tr(tr!(CreateSave), cmd(id, CREATE), mtm);
    let close = ui::button_tr(tr!(BtnClose), cmd(id, CLOSE), mtm);
    let buttons = super::buttons(&[], &close, &create, mtm);
    let parts: [(&str, &NSView); 4] = [
        (tr!(CreateSource), &source_part),
        (tr!(CreateSkip), &skip),
        (tr!(CreateProps), &props),
        (tr!(CreateOther), &other),
    ];
    let sections: Vec<Retained<objc2_app_kit::NSStackView>> =
        parts.iter().map(|(t, v)| super::section(t, v, mtm)).collect();
    let mut views: Vec<&NSView> = sections.iter().map(|s| &***s).collect();
    views.push(&progress_row);
    views.push(&buttons);
    let root = super::column(&views, mtm);
    root.setSpacing(14.0);

    // its size is its controls' (the text boxes scroll)
    let window = super::window(&ui::text(tr!(CreateTitle)), &root, None, false, mtm);
    super::focus_first(&window, &source);
    let d = Rc::new(Dialog {
        id,
        window: window.clone(),
        source,
        add_file,
        add_dir,
        skip,
        trackers,
        web_seeds,
        comment,
        piece,
        format,
        seed,
        private,
        order,
        progress,
        percent,
        create,
        close,
        remote,
        remembered: RefCell::new(r),
        cancel: Arc::new(AtomicBool::new(false)),
        shared: Arc::new(Mutex::new(Progress::default())),
        busy: Cell::new(false),
        closing: Cell::new(false),
        file: RefCell::new(None),
        seed_from: RefCell::new(None),
        created: RefCell::new(None),
        observer: RefCell::new(None),
    });
    *d.observer.borrow_mut() = Some(super::on_close(&window, id, CLOSED));
    d.sync_order();
    let handler = d.clone();
    super::set_handler(id, move |act| handler.act(act));
    super::test_accept(id, TEST);
    window.center();
    window.makeKeyAndOrderFront(None);
}

impl Dialog {
    fn act(self: &Rc<Self>, act: u8) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        match act {
            ADD_FILE | ADD_DIR => self.pick_source(act == ADD_DIR, mtm),
            CREATE => self.start(mtm),
            CLOSE if self.busy.get() => self.cancel.store(true, Ordering::Relaxed),
            CLOSE => self.window.close(),
            CLOSED => self.closed(),
            FORMAT => self.sync_order(),
            PROGRESS => self.progress(mtm),
            TEST => {
                self.pick_source(true, mtm);
                self.start(mtm);
            }
            _ => {}
        }
    }

    fn chosen_format(&self) -> TorrentFormat {
        FORMATS[ui::chosen(&self.format).unwrap_or(0).min(FORMATS.len() - 1)].1
    }

    /// "Preserve file order" applies to v1 only (v2 lays the files out sorted); a remote server
    /// has not got the files, so nothing is seeded from here.
    fn sync_order(&self) {
        self.order.setEnabled(!self.busy.get() && self.chosen_format().allows_any_order());
        if self.remote {
            ui::set_checked(&self.seed, false);
            self.seed.setEnabled(false);
        }
    }

    fn error(&self, message: &str, mtm: MainThreadMarker) {
        super::tell(Some(&self.window), ui::Style::Warning, message, "", mtm);
    }

    /// "Add file" and "Add directory".
    fn pick_source(self: &Rc<Self>, folder: bool, mtm: MainThreadMarker) {
        let now = PathBuf::from(ui::value(&self.source).trim());
        let start = if now.is_dir() { Some(now) } else { now.parent().map(Path::to_path_buf) };
        let pick = ui::Pick {
            files: !folder,
            folders: folder,
            start: start.filter(|p| p.is_dir()),
            message: ui::text(if folder { tr!(CreatePickDir) } else { tr!(PickFile) }),
            ..ui::Pick::default()
        };
        let me: Weak<Dialog> = Rc::downgrade(self);
        super::open_panel(
            Some(&self.window),
            pick,
            move |paths| {
                if let (Some(d), Some(p)) = (me.upgrade(), paths.first()) {
                    d.source.setStringValue(&ns(&p.to_string_lossy()));
                }
            },
            mtm,
        );
    }

    /// "Create and save as...": checks the source, asks where to save, then hashes.
    fn start(self: &Rc<Self>, mtm: MainThreadMarker) {
        if self.busy.get() {
            return;
        }
        let Some((opts, seed_from)) = self.collect(mtm) else { return };
        let name = create::default_file_name(&opts.source);
        let folder = create::seed_save_path(&opts.source).ok();
        let me = Rc::downgrade(self);
        // the save panel asks before replacing a file
        super::save_panel(
            Some(&self.window),
            &name,
            folder,
            move |file| {
                if let (Some(d), Some(file)) = (me.upgrade(), file) {
                    d.hash(opts, seed_from, file);
                }
            },
            mtm,
        );
    }

    /// Reads and checks the fields (and remembers them); `None` after telling the user what is
    /// wrong.
    fn collect(&self, mtm: MainThreadMarker) -> Option<(CreateOptions, Option<PathBuf>)> {
        let text = ui::value(&self.source);
        let source = PathBuf::from(text.trim().trim_matches('"'));
        if source.as_os_str().is_empty() {
            self.error(tr!(CreateNoSource), mtm);
            self.window.makeFirstResponder(Some(&self.source));
            return None;
        }
        if let Err(e) = std::fs::metadata(&source) {
            self.error(&format!("{}:\n{e}", source.display()), mtm);
            self.window.makeFirstResponder(Some(&self.source));
            return None;
        }
        let piece = ui::chosen(&self.piece).unwrap_or(0).min(PIECE_SIZES.len() - 1);
        let opts = CreateOptions {
            source: source.clone(),
            skip: ui::value(&self.skip),
            trackers: self.trackers.string().to_string(),
            web_seeds: self.web_seeds.string().to_string(),
            comment: ui::value(&self.comment),
            piece_size: PIECE_SIZES[piece],
            private: ui::checked(&self.private),
            preserve_order: ui::checked(&self.order),
            format: self.chosen_format(),
        };
        let seed = ui::checked(&self.seed);
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
                    self.error(&format!("{}:\n{e}", source.display()), mtm);
                    return None;
                }
            }
        } else {
            None
        };
        Some((opts, seed_from))
    }

    /// Hashes on a worker thread and writes `file`; the worker wakes the dialog with its progress
    /// (every tenth of a percent at most) and its end.
    fn hash(&self, opts: CreateOptions, seed_from: Option<PathBuf>, file: PathBuf) {
        *self.seed_from.borrow_mut() = seed_from;
        *self.file.borrow_mut() = Some(file.clone());
        self.cancel.store(false, Ordering::Relaxed);
        if let Ok(mut p) = self.shared.lock() {
            *p = Progress::default();
        }
        self.set_busy(true);
        let (cancel, report, id) = (self.cancel.clone(), self.shared.clone(), self.id);
        let wake = move || crate::mainq::on_main(move || super::act(id, PROGRESS));
        std::thread::spawn(move || {
            let mut shown = u32::MAX;
            let result = create::build(&opts, &mut |done, total| {
                if let Ok(mut p) = report.lock() {
                    (p.done, p.total) = (done, total);
                }
                let permille = (u64::from(done) * 1000 / u64::from(total.max(1))) as u32;
                if permille != shown {
                    shown = permille;
                    wake();
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
            wake();
        });
    }

    /// The worker's news: the bar, and its end.
    fn progress(&self, mtm: MainThreadMarker) {
        if !self.busy.get() {
            return;
        }
        let (done, total, result) = match self.shared.lock() {
            Ok(mut p) => (p.done, p.total, p.result.take()),
            Err(_) => (0, 0, Some(Err("the worker stopped".to_owned()))),
        };
        let fraction = f64::from(done) / f64::from(total.max(1));
        self.progress.setDoubleValue(fraction);
        self.percent.setStringValue(&ns(&format!("{:.1} %", fraction * 100.0)));
        if let Some(r) = result {
            self.finished(r, mtm);
        }
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        for b in [&self.add_file, &self.add_dir, &self.seed, &self.private, &self.create] {
            b.setEnabled(!busy);
        }
        for f in [&self.source, &self.skip, &self.comment] {
            f.setEnabled(!busy);
        }
        for p in [&self.piece, &self.format] {
            p.setEnabled(!busy);
        }
        for t in [&self.trackers, &self.web_seeds] {
            t.setEditable(!busy);
        }
        self.sync_order();
        self.close.setTitle(&ns(&ui::text(if busy { tr!(BtnCancel) } else { tr!(BtnClose) })));
        if busy {
            self.progress.setDoubleValue(0.0);
            self.percent.setStringValue(&ns("0.0 %"));
        }
    }

    /// The worker's end. A close that came while it wrote its last piece discards the torrent:
    /// the file it wrote is removed and nothing is added.
    fn finished(&self, result: Result<Created, String>, mtm: MainThreadMarker) {
        self.set_busy(false);
        let file = self.file.borrow_mut().take();
        let result = match result {
            Ok(_) if self.closing.get() => {
                if let Some(f) = &file {
                    let _ = std::fs::remove_file(f);
                }
                Err("cancelled".to_owned())
            }
            r => r,
        };
        match result {
            Ok(created) => {
                *self.created.borrow_mut() = Some(created);
                self.window.close();
            }
            Err(_) if self.closing.get() => self.done(),
            Err(e) => {
                self.progress.setDoubleValue(0.0);
                self.percent.setStringValue(&ns(""));
                if e != "cancelled" {
                    self.error(&tr!(CreateFailed, error = e), mtm);
                }
            }
        }
    }

    /// The window closed: while hashing, the worker is stopped first.
    fn closed(&self) {
        if self.busy.get() {
            self.cancel.store(true, Ordering::Relaxed);
            self.closing.set(true);
            return;
        }
        self.done();
    }

    /// The dialog's end: its fields are remembered, and a torrent made with "Start seeding" is
    /// added to seed.
    fn done(&self) {
        if let Some(o) = self.observer.borrow_mut().take() {
            super::forget(&o);
        }
        super::retire(self.id);
        let seed = match (self.created.borrow_mut().take(), self.seed_from.borrow_mut().take()) {
            (Some(c), Some(from)) => Some((c.bytes, from)),
            _ => None,
        };
        let r = self.remembered.borrow().clone();
        apply(with_core(|c| {
            c.created(r, seed);
            c.events()
        }));
    }
}
