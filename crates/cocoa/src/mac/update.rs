//! Rookparcel > Check for Updates (D32, D44; GTK `update_done`, `ask_open_release`): `Core`
//! asks the website on a worker thread and answers in `Events`. A newer version is offered in
//! an alert whose OK opens its release page; "up to date" and failures come as `Events`'
//! messages. The status bar's notice and its link are the main window's (slice C).
//! `ROOKPARCEL_UPDATE_URL` names another `update.txt` for a hand test.

use objc2::MainThreadMarker;
use rookparcel_frontend::app::UpdateOffer;
use rookparcel_frontend::tr;

use crate::ui;

/// Rookparcel > Check for Updates.
pub fn check() {
    crate::app::apply(crate::app::with_core(|c| {
        c.check_for_updates();
        c.events()
    }));
}

/// The update check's `Events`: a version to offer, and the status bar's notice that appeared or
/// went away (the status bar redraws it), from `app::apply`.
pub fn on_events(offer: Option<UpdateOffer>, notice: bool) {
    let _ = notice;
    let Some(offer) = offer else { return };
    let Some(mtm) = MainThreadMarker::new() else { return };
    let window = crate::app::ui().map(|u| u.window.clone()).filter(|w| w.isVisible());
    let url = offer.url();
    ui::sheet(
        window.as_deref(),
        ui::Style::Info,
        &offer.message(),
        &offer.detail(),
        &[tr!(BtnOk), tr!(BtnCancel)],
        move |b| {
            if b == 0 {
                super::about::open_url(&url);
            }
        },
        mtm,
    );
}
