//! macOS integration (slice G).

pub mod about;
pub mod assoc;
pub mod closing;
pub mod dock;
pub mod lifecycle;
pub mod login;
pub mod notify;
pub mod open;
pub mod power;
pub mod status_item;
pub mod update;

use rookparcel_frontend::integration::Integration;

/// Preferences > General's desktop integration as the system has it now (associations from
/// LaunchServices, autostart from the LaunchAgent), for Preferences (slice E) to show.
/// Not yet built (slice G).
pub fn integration() -> Integration {
    Integration::default()
}

/// Writes what differs between `old` (as `integration` read it) and `new`; `Err` is the message
/// for the user (`ErrDesktopIntegration`).
/// Not yet built (slice G).
pub fn set_integration(old: &Integration, new: &Integration) -> Result<(), String> {
    let _ = (old, new);
    crate::app::not_yet("desktop integration");
    Ok(())
}

/// Whether the associations can be changed here: from the bundle and in the default profile.
/// Not yet built (slice G).
pub fn associations_settable() -> bool {
    false
}
