//! `RPTarget`, the one action target (convention 1): menu items, toolbar items and controls
//! carry a [`Command`]'s tag and the `perform:` action; `perform:` turns the sender's tag back
//! into the command and runs it through `app::run`. `validateMenuItem:` and
//! `validateToolbarItem:` ask `app::state` whether the command is enabled (and checked), and
//! `app::title` for the items whose text follows the state (Show / Hide). It is also the menus'
//! delegate (`menuNeedsUpdate:` rebuilds the lists in File > Profiles and Torrent > Label) and
//! the timers' target (`tick:`, `snapshot:`).

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControlStateValueMixed, NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuDelegate,
    NSMenuItem, NSMenuItemValidation, NSToolbarItem, NSToolbarItemValidation,
};
use objc2_foundation::{NSString, NSTimer};

use crate::commands::Command;

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and RPTarget does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RPTarget"]
    pub struct RPTarget;

    impl RPTarget {
        /// Every menu item's, toolbar item's and control's action: runs the sender's command.
        #[unsafe(method(perform:))]
        fn perform(&self, sender: Option<&AnyObject>) {
            if let Some(cmd) = sender.and_then(command_of) {
                crate::app::run(cmd);
            }
        }

        /// The 1 s timer's action.
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&NSTimer>) {
            crate::app::tick();
        }

        /// The `ROOKPARCEL_SNAPSHOTS` timer's action.
        #[unsafe(method(snapshot:))]
        fn snapshot(&self, _timer: Option<&NSTimer>) {
            crate::snapshots::take();
        }
    }

    unsafe impl NSObjectProtocol for RPTarget {}

    unsafe impl NSMenuItemValidation for RPTarget {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            validate_menu_item(item)
        }
    }

    unsafe impl NSToolbarItemValidation for RPTarget {
        #[unsafe(method(validateToolbarItem:))]
        fn validate_toolbar_item(&self, item: &NSToolbarItem) -> bool {
            validate_toolbar_item(item)
        }
    }

    unsafe impl NSMenuDelegate for RPTarget {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            crate::menu::needs_update(menu);
        }
    }
);

fn validate_menu_item(item: &NSMenuItem) -> bool {
    let Some(cmd) = Command::from_tag(item.tag()) else { return true };
    let (enabled, checked) = crate::app::state(cmd);
    item.setState(match checked {
        Some(true) => NSControlStateValueOn,
        Some(false) | None => NSControlStateValueOff,
    });
    if let Some(title) = crate::app::title(cmd) {
        item.setTitle(&NSString::from_str(&title));
    }
    enabled
}

fn validate_toolbar_item(item: &NSToolbarItem) -> bool {
    Command::from_tag(item.tag()).is_none_or(|cmd| crate::app::state(cmd).0)
}

/// The mixed state, for tri-state check boxes elsewhere.
pub const MIXED: isize = NSControlStateValueMixed;

/// The command a sender (a menu item, toolbar item or control: all answer `tag`) carries.
fn command_of(sender: &AnyObject) -> Option<Command> {
    // SAFETY: `respondsToSelector:` is NSObject's; every object here is an NSObject.
    let answers: bool = unsafe { msg_send![sender, respondsToSelector: sel!(tag)] };
    if !answers {
        return None;
    }
    // SAFETY: the sender answers `tag`, an `NSInteger` for menu items, toolbar items and controls.
    let tag: isize = unsafe { msg_send![sender, tag] };
    Command::from_tag(tag)
}

thread_local! {
    static TARGET: OnceCell<Retained<RPTarget>> = const { OnceCell::new() };
}

/// The shared `RPTarget`, made at the first call (it lives as long as the thread).
pub fn target(mtm: MainThreadMarker) -> Retained<RPTarget> {
    TARGET.with(|t| {
        t.get_or_init(|| {
            let this = RPTarget::alloc(mtm).set_ivars(());
            // SAFETY: NSObject's `init` on a freshly allocated object with its (empty) ivars set.
            unsafe { msg_send![super(this), init] }
        })
        .clone()
    })
}

/// The `perform:` selector, for controls and items made elsewhere.
pub fn perform() -> objc2::runtime::Sel {
    sel!(perform:)
}

/// Points `item` (a menu item, toolbar item or control) at `cmd`: its tag, `RPTarget` and
/// `perform:`.
pub fn wire(item: &AnyObject, cmd: Command, mtm: MainThreadMarker) {
    let target = target(mtm);
    // SAFETY: menu items, toolbar items and controls answer `setTag:`, `setTarget:` (kept weak:
    // the shared target lives as long as the thread) and `setAction:` with these types.
    unsafe {
        let _: () = msg_send![item, setTag: cmd.tag()];
        let _: () = msg_send![item, setTarget: &*target];
        let _: () = msg_send![item, setAction: perform()];
    }
}
