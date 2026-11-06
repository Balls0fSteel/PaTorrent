//! About Rookparcel (`orderFrontStandardAboutPanelWithOptions:`: the name, the version with the
//! profile's tag, the About text, the icon), Help > Third-Party Notices (a text
//! window, as GTK's `notices.rs`: THIRD-PARTY-NOTICES.md built into the program, so the licence
//! texts stay with a binary copied out on its own) and Help > Rookparcel Website.

use std::cell::RefCell;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionCredits,
    NSApplication, NSFont, NSTextView, NSWindow, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSSize, NSString, NSURL};
use rookparcel_frontend::{lang, profile, tr};

use crate::ui::{self, layout};

/// The notices as shipped beside the binaries.
pub const NOTICES: &str = include_str!("../../../../THIRD-PARTY-NOTICES.md");

thread_local! {
    static NOTICES_WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
}

/// About Rookparcel.
pub fn about() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let name = NSString::from_str("Rookparcel");
    let version = NSString::from_str(&profile::version());
    let credits = NSAttributedString::from_nsstring(&NSString::from_str(tr!(AboutShort)));
    // SAFETY: AppKit's option keys.
    let keys = unsafe {
        [NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionCredits]
    };
    let values: [&AnyObject; 3] = [&name, &version, &credits];
    let options = NSDictionary::from_slices(&keys, &values);
    let app = NSApplication::sharedApplication(mtm);
    super::lifecycle::activate(mtm);
    // SAFETY: the keys' values are the types AppKit documents for them.
    unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
}

/// Help > Third-Party Notices: the text read-only in a resizable window.
pub fn notices() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if let Some(w) = NOTICES_WINDOW.with(|n| n.borrow().clone()) {
        w.makeKeyAndOrderFront(None);
        super::lifecycle::activate(mtm);
        return;
    }
    let scroll = NSTextView::scrollableTextView(mtm);
    ui::auto(&scroll);
    if let Some(view) = scroll.documentView().and_then(|v| v.downcast::<NSTextView>().ok()) {
        view.setEditable(false);
        view.setString(&NSString::from_str(NOTICES));
        view.setFont(Some(
            &NSFont::userFixedPitchFontOfSize(0.0).unwrap_or_else(|| NSFont::systemFontOfSize(0.0)),
        ));
        view.setTextContainerInset(NSSize::new(6.0, 6.0));
    }
    let body = layout::vstack(&[&scroll], mtm);
    layout::insets(&body, 0.0, 0.0, 0.0, 0.0);
    layout::min_width(&scroll, 520.0);
    layout::activate(&[scroll.heightAnchor().constraintGreaterThanOrEqualToConstant(360.0)]);
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::Miniaturizable;
    let window = super::window(&lang::plain(tr!(MenuNotices)), &body, style, mtm);
    window.setContentSize(NSSize::new(640.0, 480.0));
    window.center();
    window.makeKeyAndOrderFront(None);
    super::lifecycle::activate(mtm);
    NOTICES_WINDOW.with(|n| *n.borrow_mut() = Some(window));
}

/// Help > Rookparcel Website, in the default browser.
pub fn website() {
    open_url(rookparcel_frontend::update::WEBSITE_URL);
}

/// Opens `url` in the program the system has for it.
pub fn open_url(url: &str) {
    if let Some(u) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&u);
    }
}
