//! Help > Third-Party Notices: THIRD-PARTY-NOTICES.md, built into the program, so the licence
//! texts of the ported code stay with a binary copied out on its own (`patorrent-nox` prints
//! them with `--license`), as in the Win32 client. A resizable window: the text read-only in a
//! scrolled view, and Close (Escape closes too).

use gtk::prelude::*;
use patorrent_frontend::lang::{self, gtk_mnemonic};
use patorrent_frontend::tr;

/// The notices as shipped beside the binaries.
pub const NOTICES: &str = include_str!("../../../THIRD-PARTY-NOTICES.md");

/// Shows the notices over `parent`.
pub fn show(parent: &impl IsA<gtk::Window>) {
    let window = gtk::Window::builder()
        .title(lang::plain(tr!(MenuNotices)))
        .transient_for(parent)
        .modal(true)
        .default_width(640)
        .default_height(480)
        .resizable(true)
        .build();
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .monospace(true)
        .left_margin(6)
        .right_margin(6)
        .top_margin(6)
        .bottom_margin(6)
        .build();
    view.buffer().set_text(NOTICES);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&view)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(true)
        .has_frame(true)
        .build();
    let close = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnClose)));
    close.set_halign(gtk::Align::End);
    close.connect_clicked({
        let window = window.downgrade();
        move |_| {
            if let Some(w) = window.upgrade() {
                w.close();
            }
        }
    });
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(10);
    body.set_margin_end(10);
    body.append(&scroll);
    body.append(&close);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&close));
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);
    window.present();
    close.grab_focus();
}
