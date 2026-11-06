//! The Dock icon: a click with the window hidden in the status item (or minimized into it)
//! brings it back (`applicationShouldHandleReopen:hasVisibleWindows:`). The Dock menu is
//! `menu::dock`, built by `RPAppDelegate` each time it opens, and runs its commands through
//! `RPTarget` like every other menu.

/// `applicationShouldHandleReopen:hasVisibleWindows:`.
pub fn reopen(has_visible_windows: bool) -> bool {
    if !has_visible_windows {
        super::lifecycle::show_window();
    }
    true
}
