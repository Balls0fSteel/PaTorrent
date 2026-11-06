//! `NSImage`s from `frontend::icons`' embedded PNGs (D27) and from SF Symbols.
//!
//! A state or category icon is one 16-point image carrying the 16 and 32 pixel PNGs as two
//! representations, so a Retina screen draws the sharp one. The dark palette is a second image:
//! [`is_dark`] tells which one a view needs (by its effective appearance), and a view that
//! changes appearance asks again (`RPTableView` and `RPCanvas` redraw then). Images are made
//! once and kept for the thread's life.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBitmapImageRep, NSImage, NSView,
};
use objc2_foundation::{NSArray, NSData, NSSize, NSString};
use rookparcel_frontend::icons::{self, Icon, StateIcon, UiIcon};

use crate::commands::Command;

thread_local! {
    static CACHE: RefCell<HashMap<(u8, u16, bool), Retained<NSImage>>> = RefCell::new(HashMap::new());
}

/// An image of `points` points from PNGs of the given pixel sizes (any that do not decode are
/// left out).
pub fn from_pngs(points: f64, pngs: &[&[u8]]) -> Retained<NSImage> {
    let size = NSSize::new(points, points);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    for png in pngs {
        if let Some(rep) = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(png)) {
            rep.setSize(size);
            image.addRepresentation(&rep);
        }
    }
    image
}

fn cached(key: (u8, u16, bool), make: impl FnOnce() -> Retained<NSImage>) -> Retained<NSImage> {
    if let Some(i) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return i;
    }
    let i = make();
    CACHE.with(|c| c.borrow_mut().insert(key, i.clone()));
    i
}

/// A torrent state, tracker or network icon at 16 points.
pub fn state(icon: StateIcon, dark: bool) -> Retained<NSImage> {
    cached((0, icon as u16, dark), || {
        from_pngs(16.0, &[icon.themed_png(16, dark), icon.themed_png(32, dark)])
    })
}

/// A category, tab, page or menu icon at 16 points.
pub fn ui(icon: UiIcon, dark: bool) -> Retained<NSImage> {
    cached((1, icon as u16, dark), || {
        from_pngs(16.0, &[icon.themed_png(16, dark), icon.themed_png(32, dark)])
    })
}

/// A toolbar PNG at 24 points (the fallback for a missing SF Symbol).
pub fn toolbar(icon: Icon, dark: bool) -> Retained<NSImage> {
    cached((2, icon as u16, dark), || from_pngs(24.0, &[icon.png(24, dark), icon.png(48, dark)]))
}

/// The application icon at `points` points, from every exported size.
pub fn app_icon(points: f64) -> Retained<NSImage> {
    let pngs: Vec<&[u8]> = icons::APP_SIZES.iter().filter_map(|&s| icons::app_png(s)).collect();
    from_pngs(points, &pngs)
}

/// An SF Symbol (a template image, tinted by the control), described for VoiceOver.
pub fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )
}

/// The toolbar PNG standing in for a command's symbol.
fn toolbar_icon(cmd: Command) -> Option<Icon> {
    Some(match cmd {
        Command::AddTorrent => Icon::AddTorrent,
        Command::AddUrl => Icon::AddUrl,
        Command::CreateTorrent => Icon::Create,
        Command::Remove => Icon::Remove,
        Command::Start => Icon::Start,
        Command::Pause => Icon::Pause,
        Command::Stop => Icon::Stop,
        Command::QueueUp => Icon::Up,
        Command::QueueDown => Icon::Down,
        Command::Preferences => Icon::Preferences,
        Command::Rss => Icon::Rss,
        _ => return None,
    })
}

/// A command's toolbar image: its SF Symbol, else its D27 PNG.
pub fn command(cmd: Command, description: &str, mtm: MainThreadMarker) -> Option<Retained<NSImage>> {
    crate::symbols::command(cmd)
        .and_then(|n| symbol(n, description))
        .or_else(|| toolbar_icon(cmd).map(|i| toolbar(i, app_is_dark(mtm))))
}

fn dark_of(a: &NSAppearance) -> bool {
    // SAFETY: the two names are AppKit's constants.
    let names = unsafe { [NSAppearanceNameAqua, NSAppearanceNameDarkAqua] };
    let best = a.bestMatchFromAppearancesWithNames(&NSArray::from_slice(&names));
    // SAFETY: as above.
    best.is_some_and(|b| &*b == unsafe { NSAppearanceNameDarkAqua })
}

/// Whether `view` draws in a dark appearance now.
pub fn is_dark(view: &NSView) -> bool {
    dark_of(&view.effectiveAppearance())
}

/// Whether the application as a whole is dark now.
pub fn app_is_dark(mtm: MainThreadMarker) -> bool {
    dark_of(&objc2_app_kit::NSApplication::sharedApplication(mtm).effectiveAppearance())
}
