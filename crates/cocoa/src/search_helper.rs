//! The search helper (D51, `docs/SEARCH_HELPER.md`) in the Search Sources window, as the GTK and
//! Windows GUIs have it: "Get the Search Helper (Jackett)..." while it is not installed; "Pick
//! Sites...", "Open the Helper's Page", "Update the Search Helper" and "Remove the Search Helper"
//! once it is; none where this build or system does not offer it (`helper::available`). Each
//! step runs on a thread behind a small progress sheet ([`run_job`], with Cancel), on
//! `frontend::search::helper`; nothing is downloaded or started without the user's click.
//!
//! [`manage_sources`] opens the Search Sources window (`search::sources`).

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, Message};
use objc2_app_kit::{
    NSButton, NSPasteboard, NSPasteboardTypeString, NSProgressIndicator, NSProgressIndicatorStyle,
    NSScrollView, NSStackView, NSTextField, NSView, NSWindow, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSSize, NSURL};
use rookparcel_frontend::search::helper::{self, Indexer};
use rookparcel_frontend::search::view::{initially_ticked, site_detail, ticked_ids};
use rookparcel_frontend::{fmt, tr};

use crate::app::peek;
use crate::dialogs::{self, NOTHING, cmd};
use crate::search::sources::{Manager, bold};
use crate::search::{configure_helper, store, stored_sources};
use crate::ui::{self, layout, ns};

/// The Search Sources window's actions from here on are the helper's.
pub const FIRST_ACT: u8 = 10;
const GET: u8 = 10;
const UPDATE: u8 = 11;
const SITES: u8 = 12;
const PAGE: u8 = 13;
const REMOVE: u8 = 14;

/// The search field's Manage Sources... and the Search Results window's: the Search Sources
/// window.
pub fn manage_sources() {
    crate::search::sources::open();
}

/// The helper's buttons of the Search Sources window.
pub struct Buttons {
    pub row: Retained<NSStackView>,
    get: Retained<NSButton>,
    sites: Retained<NSButton>,
    page: Retained<NSButton>,
    update: Retained<NSButton>,
    remove: Retained<NSButton>,
}

impl Buttons {
    /// The buttons of the Search Sources window numbered `id`.
    pub fn new(id: u16, mtm: MainThreadMarker) -> Buttons {
        let b = |t: &str, act: u8| ui::button_tr(t, cmd(id, act), mtm);
        let get = b(tr!(SearchHelperGet), GET);
        let sites = b(tr!(SearchHelperSites), SITES);
        let page = b(tr!(SearchHelperOpenPage), PAGE);
        let update = b(tr!(SearchHelperUpdate), UPDATE);
        let remove = b(tr!(SearchHelperRemove), REMOVE);
        let row = layout::hstack(&[&get, &sites, &page, &update, &remove], mtm);
        Buttons { row, get, sites, page, update, remove }
    }

    /// Get while not installed, the other four once it is, none where it is not offered.
    pub fn show(&self) {
        let on = configure_helper();
        let installed = on && helper::installed();
        self.row.setHidden(!on);
        self.get.setHidden(installed);
        for b in [&self.sites, &self.page, &self.update, &self.remove] {
            b.setHidden(!installed);
        }
    }
}

/// A helper button of the Search Sources window `m`.
pub fn act(m: &Rc<Manager>, act: u8) {
    match act {
        GET => get(m.clone(), false),
        UPDATE => get(m.clone(), true),
        SITES => pick_sites(m.clone()),
        PAGE => open_page(m.clone()),
        REMOVE => remove(m.clone()),
        _ => {}
    }
}

// ---- jobs behind a progress sheet ------------------------------------------------------------

const JOB_CANCEL: u8 = 1;
const JOB_PROGRESS: u8 = 2;
const JOB_DONE: u8 = 3;

type Answer<T> = Option<Result<T, String>>;
/// What a job hands its answer to.
type Done<T> = Box<dyn FnOnce(Answer<T>)>;

struct Job<T> {
    id: u16,
    parent: Retained<NSWindow>,
    sheet: Retained<NSWindow>,
    bar: Retained<NSProgressIndicator>,
    amount: Retained<NSTextField>,
    cancel: Retained<NSButton>,
    stop: Arc<AtomicBool>,
    progress: Arc<Mutex<(u64, Option<u64>)>>,
    result: Arc<Mutex<Option<Result<T, String>>>>,
    done: RefCell<Option<Done<T>>>,
}

/// Runs `job` on a thread behind a progress sheet on `parent` (`title`, `name` above the bar,
/// Cancel); `done` gets its result, `None` when cancelled.
fn run_job<T: Send + 'static>(
    parent: &NSWindow,
    title: &str,
    name: &str,
    job: impl FnOnce(&mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<T, String> + Send + 'static,
    done: impl FnOnce(Answer<T>) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let id = dialogs::new_id();
    let heading = ui::label(title, mtm);
    bold(&heading);
    let label = ui::truncating_label(name, mtm);
    let bar = NSProgressIndicator::new(mtm);
    bar.setStyle(NSProgressIndicatorStyle::Bar);
    bar.setIndeterminate(true);
    ui::auto(&bar);
    // SAFETY: a nil sender, as AppKit's own callers pass.
    unsafe { bar.startAnimation(None) };
    let amount = ui::label("", mtm);
    let cancel = ui::button_tr(tr!(BtnCancel), cmd(id, JOB_CANCEL), mtm);
    cancel.setKeyEquivalent(&ns("\u{1b}"));
    let spacer = layout::spacer(mtm);
    let foot = layout::hstack(&[&amount, &spacer, &cancel], mtm);
    let views: [&NSView; 4] = [&heading, &label, &bar, &foot];
    let root = dialogs::column(&views, mtm);
    layout::min_width(&root, 380.0);
    let sheet = dialogs::window(title, &root, None, false, mtm);
    let j = Rc::new(Job {
        id,
        parent: parent.retain(),
        sheet,
        bar,
        amount,
        cancel,
        stop: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(Mutex::new((0, None))),
        result: Arc::new(Mutex::new(None)),
        done: RefCell::new(Some(Box::new(done))),
    });
    let handler = j.clone();
    dialogs::set_handler(id, move |act| handler.act(act));
    parent.beginSheet_completionHandler(&j.sheet, None);
    let (stop, progress, result) = (j.stop.clone(), j.progress.clone(), j.result.clone());
    std::thread::spawn(move || {
        let mut last: Option<Instant> = None;
        let mut report = |got: u64, total: Option<u64>| {
            if let Ok(mut p) = progress.lock() {
                *p = (got, total);
            }
            // the sheet redraws at most ten times a second
            if last.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                last = Some(Instant::now());
                crate::mainq::on_main(move || dialogs::act(id, JOB_PROGRESS));
            }
            !stop.load(Ordering::Relaxed)
        };
        let answer = std::panic::catch_unwind(AssertUnwindSafe(|| job(&mut report)))
            .unwrap_or_else(|_| Err("the worker stopped".to_owned()));
        if let Ok(mut r) = result.lock() {
            *r = Some(answer);
        }
        crate::mainq::on_main(move || dialogs::act(id, JOB_DONE));
    });
}

impl<T: 'static> Job<T> {
    fn act(&self, act: u8) {
        match act {
            JOB_CANCEL => {
                self.stop.store(true, Ordering::Relaxed);
                self.cancel.setEnabled(false);
            }
            JOB_PROGRESS => {
                let (got, total) = self.progress.lock().map(|p| *p).unwrap_or((0, None));
                if let Some(total) = total.filter(|&t| t > 0) {
                    self.bar.setIndeterminate(false);
                    self.bar.setDoubleValue((got as f64 / total as f64).min(1.0) * 100.0);
                    self.amount.setStringValue(&ns(&format!("{} / {}", fmt::size(got), fmt::size(total))));
                }
            }
            JOB_DONE => {
                self.parent.endSheet(&self.sheet);
                self.sheet.orderOut(None);
                dialogs::retire(self.id);
                let answer = self.result.lock().ok().and_then(|mut r| r.take());
                let answer = if self.stop.load(Ordering::Relaxed) { None } else { answer };
                if let Some(done) = self.done.borrow_mut().take() {
                    done(answer);
                }
            }
            _ => {}
        }
    }
}

/// A message on `w`.
fn message(w: &NSWindow, text: &str) {
    if let Some(mtm) = MainThreadMarker::new() {
        ui::sheet(Some(w), ui::Style::Info, text, "", &[], |_| {}, mtm);
    }
}

/// Asks `text` on `w` with `yes` and Cancel; `then` runs on `yes`.
fn ask(w: &NSWindow, text: &str, yes: &str, then: impl FnOnce() + 'static) {
    if let Some(mtm) = MainThreadMarker::new() {
        ui::sheet(
            Some(w),
            ui::Style::Warning,
            text,
            "",
            &[yes, tr!(BtnCancel)],
            move |b| {
                if b == 0 {
                    then();
                }
            },
            mtm,
        );
    }
}

// ---- the steps -------------------------------------------------------------------------------

/// Get (or Update): the newest release, the confirm box, the download, the source; after a first
/// install, the site picker.
fn get(m: Rc<Manager>, update: bool) {
    let w = m.window.clone();
    let proxy = peek(|c| c.proxy()).flatten();
    let p = proxy.clone();
    run_job(
        &w,
        tr!(SearchHelperChecking),
        helper::release::PROJECT_URL,
        move |_| helper::check(p),
        move |checked| {
            let release = match checked {
                None => return,
                Some(Err(e)) => return message(&m.window, &tr!(SearchHelperDownloadFailed, error = e)),
                Some(Ok(r)) => r,
            };
            let size = fmt::size(release.asset.size);
            let current = helper::state().map(|s| s.version).unwrap_or_default();
            let question = if update {
                if current == release.version {
                    return message(&m.window, &tr!(SearchHelperUpToDate, version = current));
                }
                tr!(SearchHelperUpdateConfirm, current = current, version = release.version, size = size)
            } else {
                let folder = helper::layout().map(|l| l.root.display().to_string()).unwrap_or_default();
                tr!(SearchHelperConfirm, version = release.version, size = size, folder = folder)
            };
            let w = m.window.clone();
            ask(&w, &question, tr!(BtnOk), move || install(m, release, proxy, update));
        },
    );
}

fn install(
    m: Rc<Manager>,
    release: helper::Release,
    proxy: Option<rookparcel_core::fetch::FetchProxy>,
    update: bool,
) {
    let title = tr!(SearchHelperDownloading, version = release.version);
    let w = m.window.clone();
    let name = release.asset.name.clone();
    run_job(
        &w,
        &title,
        &name,
        move |progress| helper::install(&release, proxy, progress),
        move |installed| {
            let state = match installed {
                None => return,
                Some(Err(e)) => return message(&m.window, &tr!(SearchHelperDownloadFailed, error = e)),
                Some(Ok(s)) => s,
            };
            let sources = helper::with_helper_source(&stored_sources(), &state);
            store(&sources);
            m.fill(Some(sources.len().saturating_sub(1)));
            m.helper.show();
            m.say(&tr!(SearchHelperInstalled, version = state.version), false);
            if !update {
                pick_sites(m);
            }
        },
    );
}

/// Remove: the confirm box, the helper stopped and its folder deleted, its source taken out.
fn remove(m: Rc<Manager>) {
    let w = m.window.clone();
    let folder = helper::layout().map(|l| l.root.display().to_string()).unwrap_or_default();
    let question = tr!(SearchHelperRemoveConfirm, folder = folder);
    ask(&w, &question, tr!(SearchHelperRemove), move || {
        let w = m.window.clone();
        run_job(
            &w,
            tr!(SearchHelperRemove),
            &folder,
            |_| helper::remove(),
            move |removed| {
                match removed {
                    None => {}
                    Some(Err(e)) => message(&m.window, &e),
                    Some(Ok(state)) => {
                        if let Some(state) = state {
                            store(&helper::without_helper_source(&stored_sources(), &state));
                        }
                        m.fill(None);
                        m.say(tr!(SearchHelperRemoved), false);
                    }
                }
                m.helper.show();
            },
        );
    });
}

/// Open the Helper's Page: started first, its password on the clipboard and in a box, then the
/// browser.
fn open_page(m: Rc<Manager>) {
    let w = m.window.clone();
    run_job(
        &w,
        tr!(SearchHelperStarting),
        "Jackett",
        |_| helper::page(),
        move |answer| {
            let Some(mtm) = MainThreadMarker::new() else { return };
            match answer {
                None => {}
                Some(Err(e)) => message(&m.window, &tr!(SearchHelperFailed, error = e)),
                Some(Ok((url, password))) => {
                    let board = NSPasteboard::generalPasteboard();
                    board.clearContents();
                    // SAFETY: AppKit's pasteboard type constant.
                    board.setString_forType(&ns(&password), unsafe { NSPasteboardTypeString });
                    let text = tr!(SearchHelperPagePassword, password = password);
                    ui::sheet(
                        Some(&m.window),
                        ui::Style::Info,
                        &text,
                        "",
                        &[],
                        move |_| {
                            if let Some(u) = NSURL::URLWithString(&ns(&url)) {
                                NSWorkspace::sharedWorkspace().openURL(&u);
                            }
                        },
                        mtm,
                    );
                }
            }
        },
    );
}

/// Pick Sites: the helper's public sites (it is started first), the picker, then the ticked ones
/// configured and the unticked ones removed.
fn pick_sites(m: Rc<Manager>) {
    let w = m.window.clone();
    run_job(
        &w,
        tr!(SearchHelperStarting),
        "Jackett",
        |_| helper::list_sites(),
        move |listed| {
            let list = match listed {
                None => return,
                Some(Err(e)) => return message(&m.window, &tr!(SearchHelperFailed, error = e)),
                Some(Ok(l)) => l,
            };
            let w = m.window.clone();
            picker(&w, list.clone(), move |ticked| {
                let Some(ticked) = ticked else { return };
                let count = ticked.len();
                let w = m.window.clone();
                run_job(
                    &w,
                    tr!(SearchHelperSaving),
                    "Jackett",
                    move |_| Ok(helper::apply_sites(&list, &ticked)),
                    move |saved| match saved {
                        None => {}
                        Some(Ok(Err(errors))) => {
                            message(&m.window, &tr!(SearchHelperSitesFailed, errors = errors.join("\n")))
                        }
                        Some(Err(e)) => message(&m.window, &e),
                        Some(Ok(Ok(()))) => m.say(&tr!(SearchHelperSitesDone, count = count), false),
                    },
                );
            });
        },
    );
}

const PICK_OK: u8 = 1;
const PICK_CANCEL: u8 = 2;

/// The site picker, a sheet on `parent`: a check box per public site with its language, type and
/// note (nothing ticked for the user; FlareSolverr ones greyed). `done` gets the ids ticked on
/// OK, `None` on Cancel.
fn picker(parent: &NSWindow, list: Vec<Indexer>, done: impl FnOnce(Option<Vec<String>>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let id = dialogs::new_id();
    let hint = ui::wrapping_label(&ui::text(tr!(SearchHelperSitesHint)), mtm);
    layout::width(&hint, 560.0);
    let mut checks: Vec<Retained<NSButton>> = Vec::new();
    let mut rows: Vec<Retained<NSStackView>> = Vec::new();
    for (i, on) in list.iter().zip(initially_ticked(&list)) {
        let check = ui::check(&i.name, on, cmd(id, NOTHING), mtm);
        let detail = ui::note(&site_detail(i), mtm);
        if i.needs_flaresolverr {
            check.setEnabled(false);
            ui::tooltip(&check, tr!(SearchHelperNeedsFlaresolverr));
        }
        let views: [&NSView; 2] = [&check, &detail];
        let row = layout::vstack(&views, mtm);
        row.setSpacing(2.0);
        rows.push(row);
        checks.push(check);
    }
    let row_views: Vec<&NSView> = rows.iter().map(|r| &***r).collect();
    let stack = layout::vstack(&row_views, mtm);
    layout::insets(&stack, 8.0, 8.0, 8.0, 8.0);
    let scroll = NSScrollView::new(mtm);
    ui::auto(&scroll);
    scroll.setHasVerticalScroller(true);
    scroll.setDocumentView(Some(&stack));
    let clip = scroll.contentView();
    layout::activate(&[
        stack.topAnchor().constraintEqualToAnchor(&clip.topAnchor()),
        stack.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()),
        stack.trailingAnchor().constraintEqualToAnchor(&clip.trailingAnchor()),
        scroll.heightAnchor().constraintGreaterThanOrEqualToConstant(320.0),
    ]);
    let ok = ui::button_tr(tr!(BtnOk), cmd(id, PICK_OK), mtm);
    let cancel = ui::button_tr(tr!(BtnCancel), cmd(id, PICK_CANCEL), mtm);
    let buttons = dialogs::buttons(&[], &cancel, &ok, mtm);
    let views: [&NSView; 3] = [&hint, &scroll, &buttons];
    let root = dialogs::column(&views, mtm);
    let sheet = dialogs::window(
        &ui::text(tr!(SearchHelperSitesTitle)),
        &root,
        Some(NSSize::new(600.0, 480.0)),
        false,
        mtm,
    );
    // the first sites in view
    sheet.layoutIfNeeded();
    if !stack.isFlipped() {
        let h = stack.frame().size.height - clip.bounds().size.height;
        clip.scrollToPoint(NSPoint::new(0.0, h.max(0.0)));
        scroll.reflectScrolledClipView(&clip);
    }
    let parent = parent.retain();
    let done = RefCell::new(Some(done));
    let s = sheet.clone();
    let p = parent.clone();
    dialogs::set_handler(id, move |act| {
        let answer = match act {
            PICK_OK => Some(ticked_ids(&list, &checks.iter().map(|c| ui::checked(c)).collect::<Vec<_>>())),
            PICK_CANCEL => None,
            _ => return,
        };
        p.endSheet(&s);
        s.orderOut(None);
        dialogs::retire(id);
        if let Some(f) = done.borrow_mut().take() {
            f(answer);
        }
    });
    parent.beginSheet_completionHandler(&sheet, None);
}
