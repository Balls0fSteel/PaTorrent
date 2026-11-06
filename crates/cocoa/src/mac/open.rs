//! Torrents handed to Rookparcel from outside (GTK's `open`, `PENDING` and command line):
//!
//! - `application:openURLs:` ([`open`]): `.torrent` files (Finder, `open x.torrent`, the Dock
//!   icon) and `magnet:` and `http(s)` links (a browser, `open "magnet:..."`). Those that come
//!   before `Core` exists wait ([`started`] opens them).
//! - Files and links on the command line (`Rookparcel x.torrent`).
//! - A second start of the same profile finds the library's lock held: it posts
//!   `io.github.balls0fsteel.Rookparcel.open` on the distributed notification center (object:
//!   the profile's name, empty for the default one; user info: its library folder and its files
//!   and links) and ends with status 0 ([`forward_to_running`]); the running copy opens them and
//!   comes to the front ([`started`] listens).
//!
//! A file is read and given to `Core::add_file`, a link to `Core::add_link`; what comes of it
//! goes to `dialogs::add::handle` (the Add New Torrent dialog, or a message). The window comes to
//! the front unless Preferences' "Activate the program window" (`ui.activate_on_add`) is off.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSArray, NSDictionary, NSDistributedNotificationCenter, NSNotification, NSString};
use rookparcel_core::util::lockfile;
use rookparcel_frontend::integration::mac::{Target, target};
use rookparcel_frontend::{profile, tr};

use crate::app::{peek, with_core};
use crate::{mainq, ui};

/// The distributed notification a second start posts.
const NOTIFICATION: &str = "io.github.balls0fsteel.Rookparcel.open";
/// Its user info's keys: the library folder, and the files and links.
const KEY_LIBRARY: &str = "library";
const KEY_ARGS: &str = "args";

thread_local! {
    /// What came before `Core` (opened by [`started`]).
    static PENDING: RefCell<Vec<Target>> = const { RefCell::new(Vec::new()) };
    /// This process is a second start handing everything to the running one.
    static FORWARDING: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

/// The files and links on this process's command line (after `--profile NAME`, without
/// switches and `--demo`'s number).
fn command_line() -> Vec<Target> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = profile::split_args(&args).map_or(args, |(_, rest)| rest);
    let mut out = Vec::new();
    let mut skip = false;
    for a in &args {
        if std::mem::take(&mut skip) {
            continue;
        }
        if a == "--demo" {
            skip = true;
            continue;
        }
        out.extend(target(a, &cwd()));
    }
    out
}

/// `application:openURLs:` (the URLs' absolute strings).
pub fn open(urls: Vec<String>) {
    let targets: Vec<Target> = urls.iter().filter_map(|u| target(u, &cwd())).collect();
    open_targets(targets);
}

fn open_targets(targets: Vec<Target>) {
    if targets.is_empty() {
        return;
    }
    if let Some(library) = FORWARDING.with(|f| f.borrow().clone()) {
        post(&library, &targets);
        return;
    }
    if !STARTED.with(Cell::get) {
        PENDING.with(|p| p.borrow_mut().extend(targets));
        return;
    }
    let front = peek(|c| c.gui_options().activate_on_add).unwrap_or(true);
    if front {
        super::lifecycle::show_window();
    }
    for t in targets {
        open_one(t);
    }
}

fn open_one(t: Target) {
    let step = match t {
        Target::Link(link) => with_core(|c| c.add_link(link)),
        Target::File(path) => match std::fs::read(&path) {
            Ok(bytes) => with_core(|c| c.add_file(bytes)),
            Err(e) => {
                let Some(mtm) = MainThreadMarker::new() else { return };
                let window = crate::app::ui().map(|u| u.window.clone());
                let text = tr!(ErrOpenFile, path = path.display(), error = e);
                ui::sheet(window.as_deref(), ui::Style::Error, &text, "", &[], |_| {}, mtm);
                return;
            }
        },
    };
    if let Some(step) = step {
        crate::dialogs::add::handle(step);
    }
    crate::app::apply(with_core(|c| c.events()));
}

/// The window and `Core` are there: what came before is opened (the command line's first), and
/// later starts are listened to.
pub fn started(_mtm: MainThreadMarker) {
    STARTED.with(|s| s.set(true));
    listen();
    let mut first = command_line();
    first.extend(PENDING.with(|p| p.take()));
    if !first.is_empty() {
        for t in first {
            open_one(t);
        }
    }
}

/// The library `data` is locked by another process: when a Rookparcel of this profile runs
/// there, everything to open goes to it and this process ends (status 0) half a second later,
/// so files the system opens with it right after the start follow too. False when nobody holds
/// the lock (another error).
pub fn forward_to_running(data: &Path, mtm: MainThreadMarker) -> bool {
    if !lockfile::is_held(&data.join(lockfile::FILE_NAME)) {
        return false;
    }
    FORWARDING.with(|f| *f.borrow_mut() = Some(data.to_path_buf()));
    let mut all = command_line();
    all.extend(PENDING.with(|p| p.take()));
    // even with nothing to open: the running one comes to the front
    post(data, &all);
    let _ = mtm;
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        mainq::on_main(|| {
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        });
    });
    true
}

fn profile_name() -> String {
    profile::current().name.clone().unwrap_or_default()
}

fn post(library: &Path, targets: &[Target]) {
    let args: Vec<Retained<NSString>> = targets
        .iter()
        .map(|t| match t {
            Target::File(p) => NSString::from_str(&p.to_string_lossy()),
            Target::Link(l) => NSString::from_str(l),
        })
        .collect();
    let args = NSArray::from_retained_slice(&args);
    let lib = NSString::from_str(&library.to_string_lossy());
    let keys = [&*NSString::from_str(KEY_LIBRARY), &*NSString::from_str(KEY_ARGS)];
    let values: [&AnyObject; 2] = [&lib, &args];
    let info = NSDictionary::from_slices(&keys, &values);
    // SAFETY: a dictionary of strings to objects is a dictionary of objects to objects.
    let info: Retained<NSDictionary> = unsafe { Retained::cast_unchecked(info) };
    let name = NSString::from_str(NOTIFICATION);
    let object = NSString::from_str(&profile_name());
    // SAFETY: property-list values; the object is a string, as a distributed notification needs.
    // Delivered at once even to a suspended (inactive) receiver.
    unsafe {
        NSDistributedNotificationCenter::defaultCenter()
            .postNotificationName_object_userInfo_deliverImmediately(&name, Some(&object), Some(&info), true)
    };
    eprintln!("forwarded {} file(s) and link(s) to the running Rookparcel", targets.len());
}

/// Listens for later starts of this profile.
fn listen() {
    let block = RcBlock::new(|n: NonNull<NSNotification>| {
        // SAFETY: the center hands a valid notification for the block's call.
        let n = unsafe { n.as_ref() };
        received(n);
    });
    let name = NSString::from_str(NOTIFICATION);
    let object = NSString::from_str(&profile_name());
    // SAFETY: with no queue the block runs on the thread that registered it, through its run
    // loop (the main one); the observer is kept for the process's life.
    let observer = unsafe {
        NSDistributedNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(&name),
            Some(&object),
            None,
            &block,
        )
    };
    super::lifecycle::keep_observer(observer);
}

/// A second start's files and links (for this library only: two portable copies may share a
/// profile name).
fn received(n: &NSNotification) {
    let Some(info) = n.userInfo() else { return };
    let get = |k: &str| info.objectForKey(&NSString::from_str(k));
    let library = get(KEY_LIBRARY).and_then(|o| o.downcast_ref::<NSString>().map(|s| s.to_string()));
    let mine = crate::paths::dirs().data;
    if library.as_deref().map(Path::new) != Some(mine.as_path()) {
        return;
    }
    let args: Vec<String> = get(KEY_ARGS)
        .and_then(|o| {
            o.downcast_ref::<NSArray>().map(|a| {
                a.iter().filter_map(|s| s.downcast_ref::<NSString>().map(|s| s.to_string())).collect()
            })
        })
        .unwrap_or_default();
    let targets: Vec<Target> = args.iter().filter_map(|a| target(a, Path::new("/"))).collect();
    if targets.is_empty() {
        super::lifecycle::show_window();
    } else {
        open_targets(targets);
    }
}
