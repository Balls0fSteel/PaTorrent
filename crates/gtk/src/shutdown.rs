//! Options > Auto Shutdown: the countdown window and the power actions.
//!
//! The decision lives in `frontend::auto_shutdown`; the main window runs it on its timer and
//! drives the countdown from there ([`open`], [`tick`], [`close`]), so the window only shows the
//! seconds and offers Cancel.
//!
//! Standby, Hibernate and Shut Down go to systemd-logind (`org.freedesktop.login1.Manager` on the
//! system bus: `Suspend`, `Hibernate`, `PowerOff`, interactive, so polkit may ask). Where logind
//! is missing (the BSDs, a system without systemd) or says no (`CanSuspend` and the others), the
//! menu greys them and only Quit is left. With the environment variable
//! `PATORRENT_POWER_DRY_RUN` set the action reports what it would have done instead.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_frontend::auto_shutdown::{self, Action, Mode};
use patorrent_frontend::lang::gtk_mnemonic;
use patorrent_frontend::tr;

/// The OS action after a countdown (Quit is the app's own exit).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Power {
    Standby,
    Hibernate,
    ShutDown,
}

impl Power {
    pub fn of(action: Action) -> Option<Power> {
        match action {
            Action::Quit => None,
            Action::Standby => Some(Power::Standby),
            Action::Hibernate => Some(Power::Hibernate),
            Action::ShutDown => Some(Power::ShutDown),
        }
    }

    /// logind's method, and the one that says whether it may be called.
    fn methods(self) -> (&'static str, &'static str) {
        match self {
            Power::Standby => ("Suspend", "CanSuspend"),
            Power::Hibernate => ("Hibernate", "CanHibernate"),
            Power::ShutDown => ("PowerOff", "CanPowerOff"),
        }
    }

    fn what(self) -> &'static str {
        match self {
            Power::Standby => tr!(PowerStandby),
            Power::Hibernate => tr!(PowerHibernate),
            Power::ShutDown => tr!(PowerShutDown),
        }
    }
}

fn login1(method: &str, args: Option<&glib::Variant>) -> Result<glib::Variant, glib::Error> {
    let bus = gio::bus_get_sync(gio::BusType::System, None::<&gio::Cancellable>)?;
    bus.call_sync(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
        method,
        args,
        None,
        gio::DBusCallFlags::NONE,
        5000,
        None::<&gio::Cancellable>,
    )
}

/// Whether the computer can do this; the menu greys what it cannot. "challenge" counts: polkit
/// asks for a password then.
pub fn available(action: Action) -> bool {
    let Some(p) = Power::of(action) else { return true };
    login1(p.methods().1, None)
        .ok()
        .and_then(|v| v.get::<(String,)>())
        .is_some_and(|(answer,)| answer == "yes" || answer == "challenge")
}

const DRY_RUN: &str = "PATORRENT_POWER_DRY_RUN";

/// Puts the computer on standby or into hibernation, or shuts it down. The caller has saved
/// everything first.
pub fn perform(p: Power) -> Result<(), String> {
    if std::env::var_os(DRY_RUN).is_some() {
        return Err(tr!(ShutdownDryRun, name = DRY_RUN, what = p.what()));
    }
    login1(p.methods().0, Some(&(true,).to_variant())).map(|_| ()).map_err(|e| match p {
        Power::Standby => tr!(ShutdownErrStandby, error = e),
        Power::Hibernate => tr!(ShutdownErrHibernate, error = e),
        Power::ShutDown => tr!(ShutdownErrShutDown, error = e),
    })
}

struct Open {
    window: gtk::Window,
    label: gtk::Label,
    mode: Mode,
}

thread_local! {
    /// The countdown while it is open.
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// Shut down once the app has exited (its state saved on the way out).
    static AFTER_EXIT: Cell<Option<Power>> = const { Cell::new(None) };
}

/// Asks for `p` after the main window is gone and the session has shut down.
pub fn after_exit(p: Power) {
    AFTER_EXIT.with(|a| a.set(Some(p)));
}

pub fn take_after_exit() -> Option<Power> {
    AFTER_EXIT.with(Cell::take)
}

pub fn is_open() -> bool {
    OPEN.with(|o| o.borrow().is_some())
}

/// Shows the countdown over `parent`; `cancelled` runs on the user's Cancel (or Escape, or the
/// title bar's close), not when [`close`] ends it.
pub fn open(parent: &gtk::ApplicationWindow, mode: Mode, secs: u32, cancelled: impl Fn() + 'static) {
    if is_open() {
        return;
    }
    let window = gtk::Window::builder()
        .title(tr!(ShutdownTitle))
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(340)
        .build();
    let label = gtk::Label::new(Some(&auto_shutdown::countdown_text(mode, secs)));
    label.set_xalign(0.0);
    label.set_wrap(true);
    let cancel = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnCancel)));
    cancel.set_halign(gtk::Align::End);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(14);
    body.set_margin_bottom(12);
    body.set_margin_start(14);
    body.set_margin_end(14);
    body.append(&label);
    body.append(&cancel);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&cancel));
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);
    let w = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    window.connect_close_request(move |_| {
        // still registered: the user closed it
        if OPEN.with(|o| o.borrow_mut().take()).is_some() {
            cancelled();
        }
        glib::Propagation::Proceed
    });
    OPEN.with(|o| *o.borrow_mut() = Some(Open { window: window.clone(), label, mode }));
    // over the other windows, even with the main one minimized
    window.present();
}

/// The seconds left, if the countdown is showing.
pub fn tick(secs: u32) {
    OPEN.with(|o| {
        if let Some(o) = &*o.borrow() {
            o.label.set_text(&auto_shutdown::countdown_text(o.mode, secs));
        }
    });
}

/// Ends the countdown from outside (time up, or called off by new work).
pub fn close() {
    if let Some(o) = OPEN.with(|o| o.borrow_mut().take()) {
        o.window.close();
    }
}
