//! Auto Shutdown's Sleep and Shut Down.
//!
//! Slice G fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

/// Not yet built (slice G).
pub fn auto_shutdown(index: usize) {
    let _ = index;
    crate::app::not_yet("Auto Shutdown");
}

/// Auto Shutdown's `Events`: the countdown (`auto_shutdown`), a standby or hibernation now
/// (`power`), a shutdown after the exit (`power_after_exit`), from `app::apply`.
/// Not yet built (slice G).
pub fn on_events(
    auto_shutdown: Option<rookparcel_frontend::auto_shutdown::Step>,
    power: Option<rookparcel_frontend::app::Power>,
    power_after_exit: Option<rookparcel_frontend::app::Power>,
) {
    if auto_shutdown.is_some() || power.is_some() || power_after_exit.is_some() {
        crate::app::not_yet("Auto Shutdown's countdown and power actions");
    }
}
