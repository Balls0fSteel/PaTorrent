//! The update check.
//!
//! Slice G fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

/// Not yet built (slice G).
pub fn check() {
    crate::app::not_yet("Check for Updates");
}

/// The update check's `Events`: a version to offer, and the status bar's notice that appeared or
/// went away, from `app::apply`.
/// Not yet built (slice G).
pub fn on_events(offer: Option<rookparcel_frontend::app::UpdateOffer>, notice: bool) {
    if offer.is_some() || notice {
        crate::app::not_yet("the update offer and notice");
    }
}
