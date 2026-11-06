//! The status item (the tray, as the GTK bundle's `tray_macos.rs` has it): an `NSStatusItem`
//! in the menu bar showing the colour application icon at 18 points (there is no monochrome
//! rook), the rates as its tooltip (`Core::tray_tip`). A left click shows the window or hides it
//! into the item (`lifecycle::toggle_window`); a right click or a Control-click opens
//! `menu::status_item` (Show / Hide following the window, Add Torrent, Add Torrent from URL,
//! Start All, Stop All, Quit). The button runs `Command::StatusItem` through `RPTarget`.

use std::cell::RefCell;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSEventMask, NSEventModifierFlags, NSEventType, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};

use crate::commands::Command;
use crate::{images, menu, ui};

/// The icon's size in the menu bar, in points (Apple's size for a status item's image in the
/// 22 point bar).
const ICON_POINTS: f64 = 18.0;

thread_local! {
    static ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
}

fn item() -> Option<Retained<NSStatusItem>> {
    ITEM.with(|i| i.borrow().clone())
}

/// Puts the item in the menu bar.
pub fn start(mtm: MainThreadMarker) {
    if item().is_some() {
        return;
    }
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(b) = item.button(mtm) {
        b.setImage(Some(&images::app_icon(ICON_POINTS)));
        ui::target::wire(&b, Command::StatusItem, mtm);
        // the action comes on the release of either button, as status items do
        b.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
    }
    ITEM.with(|i| *i.borrow_mut() = Some(item));
    update();
}

/// The tooltip after `Events::status` (the rates).
pub fn update() {
    let (Some(item), Some(mtm)) = (item(), MainThreadMarker::new()) else { return };
    let Some(tip) = crate::app::peek(|c| c.tray_tip()) else { return };
    if let Some(b) = item.button(mtm) {
        ui::tooltip(&b, &tip);
    }
}

/// Takes the item out of the menu bar (the application ends).
pub fn remove() {
    if let Some(item) = ITEM.with(|i| i.borrow_mut().take()) {
        NSStatusBar::systemStatusBar().removeStatusItem(&item);
    }
}

/// Whether the event that sent the action asks for the menu: a right click, or a left one with
/// Control held (none at all, as from VoiceOver's press, toggles).
fn wants_menu(kind: NSEventType, flags: NSEventModifierFlags) -> bool {
    kind == NSEventType::RightMouseUp
        || (kind == NSEventType::LeftMouseUp && flags.contains(NSEventModifierFlags::Control))
}

/// `Command::StatusItem`: the button was clicked.
pub fn clicked() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let event = NSApplication::sharedApplication(mtm).currentEvent();
    if event.is_some_and(|e| wants_menu(e.r#type(), e.modifierFlags())) {
        open_menu(mtm);
    } else {
        super::lifecycle::toggle_window();
    }
}

/// Opens the menu under the item: set for this one click (with a menu set, every click would
/// open it), its items wired to `RPTarget` and validated by `app::state` and `app::title`.
pub fn open_menu(mtm: MainThreadMarker) {
    let Some(item) = item() else { return };
    let visible = crate::app::ui().is_some_and(|u| u.window.isVisible());
    let m = menu::build("", &menu::status_item(visible), mtm);
    item.setMenu(Some(&m));
    if let Some(b) = item.button(mtm) {
        // SAFETY: the button is the item's own; a nil sender is allowed.
        unsafe { b.performClick(None) };
    }
    item.setMenu(None);
}
