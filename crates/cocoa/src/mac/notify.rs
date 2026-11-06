//! Notifications for downloads that completed (Preferences > UI Settings "Show balloon
//! notifications", `ui.balloons`; µTorrent's tray balloons; GTK `notify_completed`).
//!
//! `UNUserNotificationCenter` asks the user once (the first completion); while it waits, the
//! names queue. If it is refused or fails (as an ad-hoc-signed app may be), the older
//! `NSUserNotificationCenter` (deprecated but working on 11 to 15) takes over. Outside a bundle
//! (a bare executable has no bundle id, and `UNUserNotificationCenter` aborts without one) the
//! older one is the only one. `RPAppDelegate` is both centers' delegate and lets them show while
//! Rookparcel is in front (`willPresentNotification:`, `shouldPresentNotification:`).

use std::cell::{Cell, RefCell};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
#[allow(deprecated)]
use objc2_foundation::{NSError, NSString, NSUserNotification, NSUserNotificationCenter};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationPresentationOptions,
    UNNotificationRequest, UNUserNotificationCenter,
};
use rookparcel_frontend::tr;

use crate::mainq;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Via {
    /// Not asked yet.
    Unknown,
    /// `UNUserNotificationCenter` is asking the user.
    Asking,
    User,
    Legacy,
    /// A bare executable: no bundle id, which both centers need.
    Nowhere,
}

thread_local! {
    static VIA: Cell<Via> = const { Cell::new(Via::Unknown) };
    static QUEUE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

/// Downloads that completed (`Events::completed`), by name.
pub fn completed(names: &[String]) {
    if !crate::app::peek(|c| c.gui_options().balloons).unwrap_or(true) {
        return;
    }
    match VIA.with(Cell::get) {
        Via::User => names.iter().for_each(|n| post_user(n)),
        Via::Legacy => names.iter().for_each(|n| post_legacy(n)),
        Via::Nowhere => {}
        Via::Asking => QUEUE.with(|q| q.borrow_mut().extend(names.iter().cloned())),
        Via::Unknown => {
            QUEUE.with(|q| q.borrow_mut().extend(names.iter().cloned()));
            if super::assoc::bundle_id().is_none() {
                eprintln!("notifications: none outside Rookparcel.app (no bundle id)");
                QUEUE.with(|q| q.borrow_mut().clear());
                VIA.with(|v| v.set(Via::Nowhere));
            } else {
                VIA.with(|v| v.set(Via::Asking));
                ask();
            }
        }
    }
}

/// Asks for permission once; the answer comes on another thread.
fn ask() {
    let block = RcBlock::new(|granted: Bool, error: *mut NSError| {
        // SAFETY: null or an error valid for this call.
        let why = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
        let via = if granted.as_bool() { Via::User } else { Via::Legacy };
        let why = why.unwrap_or_else(|| if granted.as_bool() { "granted".into() } else { "refused".into() });
        mainq::on_main(move || answered(via, why));
    });
    UNUserNotificationCenter::currentNotificationCenter().requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &block,
    );
}

/// The way chosen; the queue goes out.
fn answered(via: Via, why: String) {
    eprintln!(
        "notifications: {} ({why})",
        if via == Via::User { "UNUserNotificationCenter" } else { "NSUserNotificationCenter" }
    );
    VIA.with(|v| v.set(via));
    for n in QUEUE.with(|q| q.take()) {
        if via == Via::User { post_user(&n) } else { post_legacy(&n) }
    }
}

fn next_id() -> String {
    let n = COUNT.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    format!("complete-{}-{n}", std::process::id())
}

fn post_user(name: &str) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(tr!(BalloonComplete)));
    content.setBody(&NSString::from_str(name));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&next_id()),
        &content,
        None,
    );
    let name = name.to_owned();
    let done = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: null or an error valid for this call.
        match unsafe { error.as_ref() } {
            Some(e) => {
                let why = e.localizedDescription().to_string();
                eprintln!("notifications: {name}: {why}; NSUserNotificationCenter from now on");
                let name = name.clone();
                mainq::on_main(move || {
                    VIA.with(|v| v.set(Via::Legacy));
                    post_legacy(&name);
                });
            }
            None => eprintln!("notifications: posted \"{name}\""),
        }
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, Some(&done));
}

#[allow(deprecated)]
fn post_legacy(name: &str) {
    let n = NSUserNotification::new();
    n.setTitle(Some(&NSString::from_str(tr!(BalloonComplete))));
    n.setInformativeText(Some(&NSString::from_str(name)));
    n.setIdentifier(Some(&NSString::from_str(&next_id())));
    let center: Retained<NSUserNotificationCenter> =
        NSUserNotificationCenter::defaultUserNotificationCenter();
    center.deliverNotification(&n);
    eprintln!("notifications: delivered \"{name}\" (NSUserNotificationCenter)");
}

/// What `RPAppDelegate`'s `userNotificationCenter:willPresentNotification:...` answers: shown
/// even while Rookparcel is in front.
pub fn present_options() -> UNNotificationPresentationOptions {
    UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List
}

/// Makes `RPAppDelegate` both notification centers' delegate (at the start), so banners show
/// while Rookparcel is in front whichever center posts them.
#[allow(deprecated)]
pub fn set_delegate(delegate: &crate::app::RPAppDelegate) {
    if super::assoc::bundle_id().is_none() {
        return;
    }
    UNUserNotificationCenter::currentNotificationCenter()
        .setDelegate(Some(ProtocolObject::from_ref(delegate)));
    // SAFETY: `RPAppDelegate` lives as long as the application (`main` holds it until `run`
    // returns); the center does not retain its delegate.
    unsafe {
        NSUserNotificationCenter::defaultUserNotificationCenter()
            .setDelegate(Some(ProtocolObject::from_ref(delegate)));
    }
}
