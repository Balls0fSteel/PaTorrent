//! A portable copy leaves `~/Library/Preferences` as it found it (D42: a portable copy keeps its
//! settings and state in its own folder).
//!
//! Rookparcel keeps nothing in `NSUserDefaults` itself (the window frame, toolbar and columns are
//! in `rookparcel.ini`), but AppKit does: the open and save panels write their last folders and
//! sizes (`NSNav*`, `NSOSPLastRootDirectory`) to the application's domain,
//! `~/Library/Preferences/<bundle id>.plist`. Nothing supported moves that domain elsewhere, and
//! a volatile domain only adds to what is read, never takes the writes. So a portable run copies
//! the domain at the start ([`keep`]) and puts it back at the end ([`restore`]): the plist holds
//! what it held before the run. Where there was none, cfprefsd leaves an empty one (macOS 15;
//! deleting it ourselves races with cfprefsd, which writes it again). A crash leaves the run's
//! keys behind.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSString, NSUserDefaults};
use rookparcel_frontend::profile;

/// The domain's name and its contents at the start (`None`: there was none).
type Kept = (Retained<NSString>, Option<Retained<NSDictionary<NSString, AnyObject>>>);

thread_local! {
    /// Set only in portable mode, inside the bundle.
    static KEPT: RefCell<Option<Kept>> = const { RefCell::new(None) };
}

/// At the start (`main`, before AppKit writes anything): in portable mode, the domain as it is.
pub fn keep() {
    if profile::current().portable_root.is_none() {
        return;
    }
    // a bare executable has no domain of its own to keep clean
    let Some(id) = super::assoc::bundle_id() else { return };
    let id = NSString::from_str(&id);
    let was = NSUserDefaults::standardUserDefaults().persistentDomainForName(&id);
    KEPT.with(|k| *k.borrow_mut() = Some((id, was)));
}

/// At the end (`applicationWillTerminate:`): the domain back as [`keep`] found it.
pub fn restore() {
    let Some((id, was)) = KEPT.with(|k| k.borrow_mut().take()) else { return };
    let defaults = NSUserDefaults::standardUserDefaults();
    match was {
        // SAFETY: the dictionary is the domain as read at the start: property-list values only.
        Some(d) => unsafe { defaults.setPersistentDomain_forName(&d, &id) },
        None => defaults.removePersistentDomainForName(&id),
    }
    // the process ends next: wait for the write to reach cfprefsd
    defaults.synchronize();
    eprintln!("portable: the {id} defaults are back as they were at the start");
}
