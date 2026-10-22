//! The tray on macOS (PLAN.md D23): an `NSStatusItem` in the menu bar, behind the same
//! [`crate::tray::Tray`] API as the StatusNotifierItem the other Unixes use (macOS has no D-Bus).
//!
//! - The item comes from `[NSStatusBar systemStatusBar]` with `NSVariableStatusItemLength`; its
//!   button shows the application icon (an `NSImage` of [`ICON_POINTS`] points carrying the
//!   exported PNGs from 16 to 64 pixels, so a Retina bar gets a sharp one; in colour, not a
//!   template: the icon is a coloured picture, not a glyph) and the rates as its tooltip.
//! - A left click shows the window or hides it into the tray, as the StatusNotifierItem's
//!   `Activate`; a right click (or a Control-click) opens the menu: the StatusNotifierItem's
//!   entries, Show / Hide following the window.
//! - Clicks reach Rust through one Objective-C class of our own (`PaTorrentTrayTarget`) whose
//!   two action methods look up the tray in a thread-local: AppKit calls them on the main
//!   thread, the one GDK's loop runs on. What they ask for runs from an idle callback, after
//!   AppKit's action (and the menu's tracking) has returned, so Exit never removes the item
//!   while it is still dispatching its own click.
//!
//! All of it goes through the Objective-C runtime by hand (`objc_getClass`, `sel_registerName`,
//! `objc_msgSend` cast to each signature), as `crate::macos` does for CoreServices: no `objc2`
//! or `cocoa` crate. The pure parts (which click opens the menu, the entries' titles, the
//! selectors' argument counts, the icon sizes) are compiled and tested on every system.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::ffi::CStr;

use patorrent_frontend::icons::APP_SIZES;
use patorrent_frontend::lang;

use crate::tray::{Command, MENU, entry_text};

#[cfg(target_os = "macos")]
pub use sys::Tray;

/// The icon's size in the menu bar, in points (Apple's size for a status item's image in the
/// 22 point bar).
pub const ICON_POINTS: f64 = 18.0;

/// The exported sizes of the application icon the status item's image carries, all drawn at
/// [`ICON_POINTS`]: AppKit picks the one that suits the screen's scale (20 px at 1x, 48 at 2x).
fn rep_sizes() -> impl Iterator<Item = u32> {
    APP_SIZES.into_iter().filter(|&s| s <= 64)
}

/// `NSEventTypeLeftMouseUp`.
const LEFT_MOUSE_UP: usize = 2;
/// `NSEventTypeRightMouseUp`.
const RIGHT_MOUSE_UP: usize = 4;
/// `NSEventModifierFlagControl`.
const CONTROL: usize = 1 << 18;
/// The events the button sends its action on (`sendActionOn:`): left and right clicks once
/// released, as status items do (`NSEventMaskLeftMouseUp | NSEventMaskRightMouseUp`).
const ACTION_MASK: u64 = (1 << LEFT_MOUSE_UP) | (1 << RIGHT_MOUSE_UP);

/// What a click on the item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Click {
    /// The StatusNotifierItem's `Activate`: the window to the front or into the tray.
    Toggle,
    Menu,
}

/// The click that sent the button's action, from `[NSApp currentEvent]`'s type and modifier
/// flags (none at all, as from VoiceOver's press: 0 and 0).
fn click_kind(event_type: usize, modifiers: usize) -> Click {
    let control_click = event_type == LEFT_MOUSE_UP && modifiers & CONTROL != 0;
    if event_type == RIGHT_MOUSE_UP || control_click { Click::Menu } else { Click::Toggle }
}

/// What a click asks the owner for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Activate,
    Command(Command),
}

/// Whether PaTorrent becomes the active application after `act`, when the window is on
/// screen then: a status item's click leaves the frontmost application in front, so a window
/// that was shown (or a dialog over it) would open behind it.
fn brings_forward(act: Act) -> bool {
    matches!(act, Act::Activate | Act::Command(Command::Toggle | Command::AddTorrent | Command::AddUrl))
}

/// The command of the menu entry tagged `tag` (its index in [`MENU`]).
fn command_of_tag(tag: isize) -> Option<Command> {
    usize::try_from(tag).ok().and_then(|i| MENU.get(i)).copied().flatten()
}

/// An entry's title: menus on macOS have no mnemonics.
fn menu_title(c: Command, window_visible: bool) -> String {
    lang::plain(entry_text(c, window_visible))
}

/// The arguments a selector takes: one per colon.
fn arity(name: &CStr) -> usize {
    name.to_bytes().iter().filter(|&&b| b == b':').count()
}

/// The class and the selectors this file uses.
mod names {
    use std::ffi::CStr;

    /// Our target class, an `NSObject` with [`CLICK`] and [`ENTRY`].
    pub const TARGET_CLASS: &CStr = c"PaTorrentTrayTarget";
    /// The button's action.
    pub const CLICK: &CStr = c"patorrentTrayClick:";
    /// The menu entries' action.
    pub const ENTRY: &CStr = c"patorrentTrayEntry:";
    /// Both actions' type encoding: `void (id self, SEL _cmd, id sender)`.
    pub const ACTION_TYPES: &CStr = c"v@:@";

    pub const ALLOC: &CStr = c"alloc";
    pub const INIT: &CStr = c"init";
    pub const RETAIN: &CStr = c"retain";
    pub const RELEASE: &CStr = c"release";
    pub const RESPONDS_TO_SELECTOR: &CStr = c"respondsToSelector:";

    pub const SHARED_APPLICATION: &CStr = c"sharedApplication";
    pub const CURRENT_EVENT: &CStr = c"currentEvent";
    pub const TYPE: &CStr = c"type";
    pub const MODIFIER_FLAGS: &CStr = c"modifierFlags";
    /// macOS 14 and later.
    pub const ACTIVATE: &CStr = c"activate";
    /// Before macOS 14 (deprecated there, still working).
    pub const ACTIVATE_IGNORING_OTHER_APPS: &CStr = c"activateIgnoringOtherApps:";

    pub const SYSTEM_STATUS_BAR: &CStr = c"systemStatusBar";
    pub const STATUS_ITEM_WITH_LENGTH: &CStr = c"statusItemWithLength:";
    pub const REMOVE_STATUS_ITEM: &CStr = c"removeStatusItem:";
    pub const BUTTON: &CStr = c"button";
    pub const SET_MENU: &CStr = c"setMenu:";
    pub const SET_IMAGE: &CStr = c"setImage:";
    pub const SET_TOOL_TIP: &CStr = c"setToolTip:";
    pub const SET_TARGET: &CStr = c"setTarget:";
    pub const SET_ACTION: &CStr = c"setAction:";
    pub const SEND_ACTION_ON: &CStr = c"sendActionOn:";
    pub const PERFORM_CLICK: &CStr = c"performClick:";

    pub const INIT_WITH_SIZE: &CStr = c"initWithSize:";
    pub const DATA_WITH_BYTES_LENGTH: &CStr = c"dataWithBytes:length:";
    pub const IMAGE_REP_WITH_DATA: &CStr = c"imageRepWithData:";
    pub const SET_SIZE: &CStr = c"setSize:";
    pub const ADD_REPRESENTATION: &CStr = c"addRepresentation:";
    #[cfg_attr(not(test), allow(dead_code))]
    pub const REPRESENTATIONS: &CStr = c"representations";
    #[cfg_attr(not(test), allow(dead_code))]
    pub const COUNT: &CStr = c"count";
    #[cfg_attr(not(test), allow(dead_code))]
    pub const IS_VALID: &CStr = c"isValid";

    pub const INIT_WITH_BYTES_LENGTH_ENCODING: &CStr = c"initWithBytes:length:encoding:";
    #[cfg_attr(not(test), allow(dead_code))]
    pub const UTF8_STRING: &CStr = c"UTF8String";

    pub const INIT_WITH_TITLE: &CStr = c"initWithTitle:";
    pub const SET_AUTOENABLES_ITEMS: &CStr = c"setAutoenablesItems:";
    pub const ADD_ITEM: &CStr = c"addItem:";
    pub const SEPARATOR_ITEM: &CStr = c"separatorItem";
    pub const INIT_WITH_TITLE_ACTION_KEY_EQUIVALENT: &CStr = c"initWithTitle:action:keyEquivalent:";
    pub const SET_TAG: &CStr = c"setTag:";
    pub const TAG: &CStr = c"tag";
    pub const SET_TITLE: &CStr = c"setTitle:";
}

/// The Objective-C runtime, AppKit and Foundation.
#[cfg(target_os = "macos")]
mod sys {
    use std::cell::RefCell;
    use std::ffi::{CStr, c_char, c_void};
    use std::ptr::null_mut;
    use std::rc::{Rc, Weak};

    use gtk::glib;
    use patorrent_frontend::icons::app_png;

    use super::names::*;
    use super::{
        ACTION_MASK, Act, Click, ICON_POINTS, brings_forward, click_kind, command_of_tag, menu_title,
        rep_sizes,
    };
    use crate::tray::{Command, Hooks, MENU};

    /// `id`.
    type Id = *mut c_void;
    /// `SEL`.
    type Sel = *const c_void;

    /// `BOOL`: C's `bool` on arm64, a `signed char` on x86_64.
    #[cfg(target_arch = "aarch64")]
    type Bool = bool;
    #[cfg(not(target_arch = "aarch64"))]
    type Bool = i8;
    #[cfg(target_arch = "aarch64")]
    const YES: Bool = true;
    #[cfg(not(target_arch = "aarch64"))]
    const YES: Bool = 1;
    #[cfg(target_arch = "aarch64")]
    const NO: Bool = false;
    #[cfg(not(target_arch = "aarch64"))]
    const NO: Bool = 0;

    /// `NSSize` (`CGSize`: two `CGFloat`s, doubles on 64-bit), passed by value: in two floating
    /// point registers on both architectures.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSSize {
        width: f64,
        height: f64,
    }

    /// `NSVariableStatusItemLength`.
    const VARIABLE_LENGTH: f64 = -1.0;
    /// `NSUTF8StringEncoding`.
    const UTF8: usize = 4;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        /// Declared without a signature: [`send!`] casts it to the one of each message.
        fn objc_msgSend();
        fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra_bytes: usize) -> Id;
        fn class_addMethod(class: Id, name: Sel, imp: *const c_void, types: *const c_char) -> Bool;
        fn objc_registerClassPair(class: Id);
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    // the classes are looked up by name at run time; these make sure they are loaded
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}

    /// Sends the message `$name` (a `&CStr` from `names`) to `$obj`, with each argument given as
    /// `value => type`, and `; type` for what it returns (nothing when left out).
    /// `objc_msgSend` is called through a pointer of the method's own signature: what arm64
    /// requires, and right on x86_64 for every return here (objects, integers, `BOOL`, void; no
    /// structure returns, which would need `objc_msgSend_stret`). Debug builds check that the
    /// selector takes as many arguments as are passed. Needs an `unsafe` block: the receiver
    /// must answer the message with that signature.
    macro_rules! send {
        ($obj:expr, $name:expr $(, $arg:expr => $ty:ty)* $(; $ret:ty)?) => {{
            let name: &CStr = $name;
            debug_assert_eq!(crate::tray_macos::arity(name), <[&str]>::len(&[$(stringify!($ty)),*]), "{name:?}");
            let f = std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel $(, $ty)*) $(-> $ret)?>(
                objc_msgSend,
            );
            f($obj, sel(name) $(, $arg)*)
        }};
    }

    fn sel(name: &CStr) -> Sel {
        // SAFETY: a NUL-terminated string; selectors are interned for the program's life.
        unsafe { sel_registerName(name.as_ptr()) }
    }

    fn class(name: &CStr) -> Id {
        // SAFETY: a NUL-terminated string; null when no such class is loaded.
        unsafe { objc_getClass(name.as_ptr()) }
    }

    fn truth(b: Bool) -> bool {
        b != NO
    }

    /// An autorelease pool for the objects AppKit hands back autoreleased, drained when dropped.
    /// GDK's loop drives AppKit's events itself, so a callback of ours cannot count on one.
    struct Pool(*mut c_void);

    impl Pool {
        fn new() -> Pool {
            // SAFETY: pushes a pool on this thread; `drop` pops it, in order.
            Pool(unsafe { objc_autoreleasePoolPush() })
        }
    }

    impl Drop for Pool {
        fn drop(&mut self) {
            // SAFETY: the pool pushed by `new` on this thread.
            unsafe { objc_autoreleasePoolPop(self.0) }
        }
    }

    /// Gives up our reference to `obj` (nil is fine: a message to nil does nothing).
    ///
    /// # Safety
    /// `obj` is an object we own a reference to, or nil.
    unsafe fn release(obj: Id) {
        // SAFETY: `release` takes nothing and returns nothing; the reference is ours (caller).
        unsafe { send!(obj, RELEASE) }
    }

    /// A new `NSString` of `s`, owned (released by the caller).
    fn ns_string(s: &str) -> Id {
        // SAFETY: `+alloc` and `-initWithBytes:length:encoding:` with the bytes and length of
        // one &str, which is UTF-8.
        unsafe {
            let alloc = send!(class(c"NSString"), ALLOC; Id);
            send!(alloc, INIT_WITH_BYTES_LENGTH_ENCODING,
                s.as_ptr().cast::<c_void>() => *const c_void, s.len() => usize, UTF8 => usize; Id)
        }
    }

    /// The application icon as an `NSImage` of [`ICON_POINTS`], owned.
    fn app_image() -> Id {
        let size = NSSize { width: ICON_POINTS, height: ICON_POINTS };
        // SAFETY: `-[NSImage initWithSize:]` takes an NSSize and returns the image (+1).
        let image = unsafe { send!(send!(class(c"NSImage"), ALLOC; Id), INIT_WITH_SIZE, size => NSSize; Id) };
        if image.is_null() {
            return image;
        }
        for px in rep_sizes() {
            let Some(png) = app_png(px) else { continue };
            // SAFETY: `+[NSData dataWithBytes:length:]` copies the PNG's bytes (autoreleased,
            // inside the caller's pool); `+[NSBitmapImageRep imageRepWithData:]` decodes them
            // (nil if it cannot); `-setSize:` takes an NSSize; the image retains the rep.
            unsafe {
                let data = send!(class(c"NSData"), DATA_WITH_BYTES_LENGTH,
                    png.as_ptr().cast::<c_void>() => *const c_void, png.len() => usize; Id);
                let rep = send!(class(c"NSBitmapImageRep"), IMAGE_REP_WITH_DATA, data => Id; Id);
                if rep.is_null() {
                    continue;
                }
                send!(rep, SET_SIZE, size => NSSize);
                send!(image, ADD_REPRESENTATION, rep => Id);
            }
        }
        image
    }

    /// Our target class, registered with the runtime the first time.
    fn target_class() -> Option<Id> {
        let existing = class(TARGET_CLASS);
        if !existing.is_null() {
            return Some(existing);
        }
        let click: extern "C" fn(Id, Sel, Id) = on_click;
        let entry: extern "C" fn(Id, Sel, Id) = on_entry;
        // SAFETY: a new NSObject subclass; both methods are `void (id, SEL, id)` functions,
        // which is what ACTION_TYPES says, and live for the program.
        unsafe {
            let cls = objc_allocateClassPair(class(c"NSObject"), TARGET_CLASS.as_ptr(), 0);
            if cls.is_null() {
                return None;
            }
            let added =
                truth(class_addMethod(cls, sel(CLICK), click as *const c_void, ACTION_TYPES.as_ptr()))
                    && truth(class_addMethod(cls, sel(ENTRY), entry as *const c_void, ACTION_TYPES.as_ptr()));
            objc_registerClassPair(cls);
            added.then_some(cls)
        }
    }

    thread_local! {
        /// The tray the target's methods call back into.
        static CURRENT: RefCell<Weak<Tray>> = const { RefCell::new(Weak::new()) };
    }

    fn current() -> Option<Rc<Tray>> {
        CURRENT.with(|c| c.borrow().upgrade())
    }

    /// The button's action: a left click toggles the window, a right or Control-click opens
    /// the menu.
    extern "C" fn on_click(_this: Id, _cmd: Sel, _sender: Id) {
        let Some(tray) = current() else { return };
        let _pool = Pool::new();
        // SAFETY: `+[NSApplication sharedApplication]` and `-currentEvent` return objects (the
        // event may be nil); `-type` and `-modifierFlags` return NSUIntegers.
        let (kind, modifiers) = unsafe {
            let app = send!(class(c"NSApplication"), SHARED_APPLICATION; Id);
            let event = send!(app, CURRENT_EVENT; Id);
            if event.is_null() {
                (0, 0)
            } else {
                (send!(event, TYPE; usize), send!(event, MODIFIER_FLAGS; usize))
            }
        };
        match click_kind(kind, modifiers) {
            Click::Toggle => tray.later(Act::Activate),
            Click::Menu => tray.pop_up_menu(),
        }
    }

    /// A menu entry's action; the entry's tag is its index in [`MENU`].
    extern "C" fn on_entry(_this: Id, _cmd: Sel, sender: Id) {
        let Some(tray) = current() else { return };
        // SAFETY: the sender is one of our NSMenuItems; `-tag` returns an NSInteger.
        let tag = unsafe { send!(sender, TAG; isize) };
        if let Some(c) = command_of_tag(tag) {
            tray.later(Act::Command(c));
        }
    }

    /// Makes PaTorrent the active application.
    fn activate_app() {
        // SAFETY: `-respondsToSelector:` takes a SEL and returns BOOL; `-activate` takes
        // nothing, `-activateIgnoringOtherApps:` a BOOL.
        unsafe {
            let app = send!(class(c"NSApplication"), SHARED_APPLICATION; Id);
            if truth(send!(app, RESPONDS_TO_SELECTOR, sel(ACTIVATE) => Sel; Bool)) {
                send!(app, ACTIVATE);
            } else {
                send!(app, ACTIVATE_IGNORING_OTHER_APPS, YES => Bool);
            }
        }
    }

    pub struct Tray {
        hooks: Hooks,
        /// The `NSStatusItem`, retained: the status bar does not keep it alive.
        item: Id,
        /// The item's `NSStatusBarButton` (the item owns it).
        button: Id,
        /// The `NSMenu`, owned; it owns its entries, listed with their commands.
        menu: Id,
        entries: Vec<(Command, Id)>,
        /// The `PaTorrentTrayTarget` instance, owned (buttons and entries do not retain their
        /// target).
        target: Id,
        tooltip: RefCell<String>,
    }

    impl Tray {
        /// Puts the status item in the menu bar; `None` when AppKit will not give one.
        pub fn start(hooks: Hooks) -> Option<Rc<Tray>> {
            let _pool = Pool::new();
            let target_class = target_class()?;
            // SAFETY: each message below is sent to an object of the class that declares it,
            // with the arguments and return type of its declaration; null results are checked
            // before they are used, and every reference taken (alloc, retain) is released on
            // the way out or by `drop`.
            unsafe {
                let bar = send!(class(c"NSStatusBar"), SYSTEM_STATUS_BAR; Id);
                if bar.is_null() {
                    return None;
                }
                let item = send!(bar, STATUS_ITEM_WITH_LENGTH, VARIABLE_LENGTH => f64; Id);
                if item.is_null() {
                    return None;
                }
                send!(item, RETAIN; Id);
                let button = send!(item, BUTTON; Id);
                let target = send!(send!(target_class, ALLOC; Id), INIT; Id);
                if button.is_null() || target.is_null() {
                    send!(bar, REMOVE_STATUS_ITEM, item => Id);
                    release(item);
                    release(target);
                    return None;
                }
                let image = app_image();
                send!(button, SET_IMAGE, image => Id);
                release(image);
                send!(button, SET_TARGET, target => Id);
                send!(button, SET_ACTION, sel(CLICK) => Sel);
                send!(button, SEND_ACTION_ON, ACTION_MASK => u64; isize);

                let title = ns_string("");
                let menu = send!(send!(class(c"NSMenu"), ALLOC; Id), INIT_WITH_TITLE, title => Id; Id);
                send!(menu, SET_AUTOENABLES_ITEMS, NO => Bool);
                let mut entries = Vec::new();
                for (tag, entry) in MENU.iter().enumerate() {
                    let Some(c) = *entry else {
                        send!(menu, ADD_ITEM, send!(class(c"NSMenuItem"), SEPARATOR_ITEM; Id) => Id);
                        continue;
                    };
                    let alloc = send!(class(c"NSMenuItem"), ALLOC; Id);
                    let it = send!(alloc, INIT_WITH_TITLE_ACTION_KEY_EQUIVALENT,
                        title => Id, sel(ENTRY) => Sel, title => Id; Id);
                    send!(it, SET_TARGET, target => Id);
                    send!(it, SET_TAG, tag as isize => isize);
                    send!(menu, ADD_ITEM, it => Id);
                    // the menu holds it now
                    release(it);
                    entries.push((c, it));
                }
                release(title);

                let tray = Rc::new(Tray {
                    hooks,
                    item,
                    button,
                    menu,
                    entries,
                    target,
                    tooltip: RefCell::new(String::new()),
                });
                tray.set_tooltip("PaTorrent");
                tray.menu_changed();
                CURRENT.with(|c| *c.borrow_mut() = Rc::downgrade(&tray));
                // as the StatusNotifierItem's: after `start` has returned and the owner keeps it
                let me = Rc::downgrade(&tray);
                glib::idle_add_local_once(move || {
                    if let Some(t) = me.upgrade() {
                        (t.hooks.shown)(true);
                    }
                });
                Some(tray)
            }
        }

        /// The item is in the menu bar from [`Tray::start`] on.
        pub fn shown(&self) -> bool {
            true
        }

        /// The button's tooltip: the rates under the title.
        pub fn set_tooltip(&self, text: &str) {
            if *self.tooltip.borrow() == text {
                return;
            }
            text.clone_into(&mut self.tooltip.borrow_mut());
            let _pool = Pool::new();
            let tip = ns_string(text);
            // SAFETY: `-setToolTip:` takes an NSString (copied); ours is released after.
            unsafe {
                send!(self.button, SET_TOOL_TIP, tip => Id);
                release(tip);
            }
        }

        /// The window was shown or hidden: the menu's titles follow (the menu also refreshes
        /// them each time it opens).
        pub fn menu_changed(&self) {
            let _pool = Pool::new();
            let visible = (self.hooks.window_visible)();
            for &(c, it) in &self.entries {
                let title = ns_string(&menu_title(c, visible));
                // SAFETY: `-[NSMenuItem setTitle:]` takes an NSString (copied); ours is
                // released after.
                unsafe {
                    send!(it, SET_TITLE, title => Id);
                    release(title);
                }
            }
        }

        /// Opens the menu under the item, as a click on it would with the menu attached; it
        /// is attached only meanwhile, so that a left click stays the button's action.
        fn pop_up_menu(&self) {
            self.menu_changed();
            // SAFETY: `-[NSStatusItem setMenu:]` takes an NSMenu or nil, `-performClick:` a
            // sender (nil); the menu tracks modally inside `performClick:`.
            unsafe {
                send!(self.item, SET_MENU, self.menu => Id);
                send!(self.button, PERFORM_CLICK, null_mut() => Id);
                send!(self.item, SET_MENU, null_mut() => Id);
            }
        }

        /// Runs `act` from GLib's loop, once AppKit's action has returned.
        fn later(self: &Rc<Self>, act: Act) {
            let me = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(t) = me.upgrade() {
                    t.run(act);
                }
            });
        }

        fn run(&self, act: Act) {
            match act {
                Act::Activate => (self.hooks.activate)(),
                Act::Command(c) => (self.hooks.command)(c),
            }
            if brings_forward(act) && (self.hooks.window_visible)() {
                activate_app();
            }
        }
    }

    impl Drop for Tray {
        /// Takes the item out of the menu bar.
        fn drop(&mut self) {
            let _pool = Pool::new();
            // SAFETY: `-removeStatusItem:` takes our item; the target is cleared before the
            // target object goes; the item, the menu (with its entries) and the target are
            // released once, by their owner (us).
            unsafe {
                let bar = send!(class(c"NSStatusBar"), SYSTEM_STATUS_BAR; Id);
                send!(bar, REMOVE_STATUS_ITEM, self.item => Id);
                send!(self.button, SET_TARGET, null_mut() => Id);
                release(self.item);
                release(self.menu);
                release(self.target);
            }
        }
    }

    // On the macOS CI runner, off the main thread and without NSApplication: only what AppKit
    // allows there (no status item, no menu).
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_classes_are_loaded() {
            for name in [c"NSStatusBar", c"NSMenu", c"NSMenuItem", c"NSImage", c"NSBitmapImageRep", c"NSData"]
            {
                assert!(!class(name).is_null(), "{name:?}");
            }
        }

        #[test]
        fn the_target_answers_both_actions() {
            let _pool = Pool::new();
            let cls = target_class().expect("registered");
            assert_eq!(target_class(), Some(cls), "registered once");
            // SAFETY: an instance of our class; `respondsToSelector:` takes a SEL.
            unsafe {
                let target = send!(send!(cls, ALLOC; Id), INIT; Id);
                assert!(!target.is_null());
                for name in [CLICK, ENTRY] {
                    assert!(truth(send!(target, RESPONDS_TO_SELECTOR, sel(name) => Sel; Bool)), "{name:?}");
                }
                assert!(!truth(send!(target, RESPONDS_TO_SELECTOR, sel(c"noSuchAction:") => Sel; Bool)));
                release(target);
            }
        }

        #[test]
        fn the_icon_becomes_an_image_with_every_size() {
            let _pool = Pool::new();
            let image = app_image();
            assert!(!image.is_null());
            // SAFETY: an NSImage; `-representations` returns an NSArray, `-count` an NSUInteger.
            unsafe {
                assert!(truth(send!(image, IS_VALID; Bool)));
                let reps = send!(image, REPRESENTATIONS; Id);
                assert_eq!(send!(reps, COUNT; usize), rep_sizes().count());
                release(image);
            }
        }

        #[test]
        fn strings_survive_the_round_trip() {
            let _pool = Pool::new();
            for text in ["PaTorrent\nD: 1.2 kB/s U: 0 B/s", "", "Показать PaTorrent «µ»"] {
                let s = ns_string(text);
                assert!(!s.is_null());
                // SAFETY: an NSString; `-UTF8String` returns a C string that lives as long as
                // the pool.
                unsafe {
                    let utf8 = send!(s, UTF8_STRING; *const c_char);
                    assert_eq!(CStr::from_ptr(utf8).to_str(), Ok(text));
                    release(s);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::names::*;
    use super::*;

    #[test]
    fn a_right_or_control_click_opens_the_menu() {
        assert_eq!(click_kind(LEFT_MOUSE_UP, 0), Click::Toggle);
        assert_eq!(click_kind(LEFT_MOUSE_UP, CONTROL), Click::Menu);
        // Shift, Option or Command with the click: still the click
        assert_eq!(click_kind(LEFT_MOUSE_UP, (1 << 17) | (1 << 19) | (1 << 20)), Click::Toggle);
        assert_eq!(click_kind(RIGHT_MOUSE_UP, 0), Click::Menu);
        assert_eq!(click_kind(RIGHT_MOUSE_UP, CONTROL), Click::Menu);
        // no event (VoiceOver's press, `performClick:`)
        assert_eq!(click_kind(0, 0), Click::Toggle);
        assert_eq!(ACTION_MASK, 0b1_0100);
    }

    #[test]
    fn entries_are_tagged_by_their_place() {
        for (tag, entry) in MENU.iter().enumerate() {
            assert_eq!(command_of_tag(tag as isize), *entry);
        }
        assert_eq!(command_of_tag(-1), None);
        assert_eq!(command_of_tag(MENU.len() as isize), None);
    }

    #[test]
    fn titles_have_no_mnemonics_and_follow_the_window() {
        for c in MENU.into_iter().flatten() {
            for visible in [true, false] {
                let title = menu_title(c, visible);
                assert!(!title.is_empty() && !title.contains('&'), "{c:?}: {title}");
            }
        }
        assert_ne!(menu_title(Command::Toggle, true), menu_title(Command::Toggle, false));
        assert_eq!(menu_title(Command::Toggle, true), lang::plain(patorrent_frontend::tr!(MenuHide)));
    }

    #[test]
    fn the_window_comes_forward_only_when_shown() {
        assert!(brings_forward(Act::Activate));
        assert!(brings_forward(Act::Command(Command::Toggle)));
        assert!(brings_forward(Act::Command(Command::AddTorrent)));
        assert!(brings_forward(Act::Command(Command::AddUrl)));
        assert!(!brings_forward(Act::Command(Command::StartAll)));
        assert!(!brings_forward(Act::Command(Command::StopAll)));
        assert!(!brings_forward(Act::Command(Command::Exit)));
    }

    #[test]
    fn the_icon_has_a_size_for_each_scale() {
        let sizes: Vec<u32> = rep_sizes().collect();
        assert!(!sizes.is_empty());
        // at least the points at 1x and twice them at 2x, so AppKit never scales up
        assert!(sizes.iter().any(|&s| f64::from(s) >= ICON_POINTS));
        assert!(sizes.iter().any(|&s| f64::from(s) >= 2.0 * ICON_POINTS));
        for s in sizes {
            assert!(patorrent_frontend::icons::app_png(s).is_some(), "{s}");
        }
    }

    #[test]
    fn selectors_take_as_many_arguments_as_their_colons_say() {
        let table: [(&CStr, usize); 39] = [
            (CLICK, 1),
            (ENTRY, 1),
            (ALLOC, 0),
            (INIT, 0),
            (RETAIN, 0),
            (RELEASE, 0),
            (RESPONDS_TO_SELECTOR, 1),
            (SHARED_APPLICATION, 0),
            (CURRENT_EVENT, 0),
            (TYPE, 0),
            (MODIFIER_FLAGS, 0),
            (ACTIVATE, 0),
            (ACTIVATE_IGNORING_OTHER_APPS, 1),
            (SYSTEM_STATUS_BAR, 0),
            (STATUS_ITEM_WITH_LENGTH, 1),
            (REMOVE_STATUS_ITEM, 1),
            (BUTTON, 0),
            (SET_MENU, 1),
            (SET_IMAGE, 1),
            (SET_TOOL_TIP, 1),
            (SET_TARGET, 1),
            (SET_ACTION, 1),
            (SEND_ACTION_ON, 1),
            (PERFORM_CLICK, 1),
            (INIT_WITH_SIZE, 1),
            (DATA_WITH_BYTES_LENGTH, 2),
            (IMAGE_REP_WITH_DATA, 1),
            (SET_SIZE, 1),
            (ADD_REPRESENTATION, 1),
            (REPRESENTATIONS, 0),
            (COUNT, 0),
            (IS_VALID, 0),
            (INIT_WITH_BYTES_LENGTH_ENCODING, 3),
            (UTF8_STRING, 0),
            (INIT_WITH_TITLE, 1),
            (SET_AUTOENABLES_ITEMS, 1),
            (ADD_ITEM, 1),
            (SEPARATOR_ITEM, 0),
            (INIT_WITH_TITLE_ACTION_KEY_EQUIVALENT, 3),
        ];
        for (name, args) in table {
            assert_eq!(arity(name), args, "{name:?}");
            let text = name.to_str().expect("ASCII");
            assert!(text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b':'), "{text}");
            // a selector with arguments ends with the last one's colon
            assert_eq!(text.ends_with(':'), args > 0, "{text}");
        }
        for (name, args) in [(SET_TAG, 1), (TAG, 0), (SET_TITLE, 1)] {
            assert_eq!(arity(name), args, "{name:?}");
        }
        // the actions' type encoding: returns void, takes self, _cmd and one object
        assert_eq!(ACTION_TYPES.to_bytes(), b"v@:@");
        assert_eq!(arity(CLICK) + 2, ACTION_TYPES.to_bytes().len() - 1);
        assert!(TARGET_CLASS.to_bytes().iter().all(u8::is_ascii_alphanumeric));
    }
}
