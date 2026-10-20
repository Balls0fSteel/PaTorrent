//! Options > Preferences: µTorrent 2.2.1's dialog, the page list on the left and the page on the
//! right, OK / Cancel / Apply. Only the General page's language so far; the other pages join
//! with Phase 7's dialogs (`frontend::options` already has what they edit).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use patorrent_frontend::lang::{self, gtk_mnemonic};
use patorrent_frontend::options::GuiOptions;
use patorrent_frontend::tr;

/// Shows the dialog over `parent`, editing `options` as stored; `apply` stores the changed
/// options (OK and Apply).
pub fn show(parent: &gtk::ApplicationWindow, options: GuiOptions, apply: impl Fn(&GuiOptions) + 'static) {
    let window = gtk::Window::builder()
        .title(tr!(PrefTitle))
        .transient_for(parent)
        .modal(true)
        .default_width(620)
        .default_height(400)
        .build();

    // General > Language
    let languages = lang::choices(&options.language, &lang::system_languages());
    let names: Vec<&str> = languages.iter().map(|(_, name)| name.as_str()).collect();
    let language = gtk::DropDown::from_strings(&names);
    let at = languages.iter().position(|(code, _)| *code == options.language).unwrap_or(0);
    language.set_selected(at as u32);
    language.set_hexpand(true);
    let label = gtk::Label::with_mnemonic(&gtk_mnemonic(tr!(PrefUiLanguage)));
    label.set_mnemonic_widget(Some(&language));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_top(8);
    row.set_margin_bottom(8);
    row.set_margin_start(8);
    row.set_margin_end(8);
    row.append(&label);
    row.append(&language);
    let frame = gtk::Frame::new(Some(tr!(PrefLanguage)));
    frame.set_child(Some(&row));
    let general = gtk::Box::new(gtk::Orientation::Vertical, 12);
    general.append(&frame);

    // the page list, as µTorrent's tree
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.add_titled(&general, Some("general"), tr!(PageGeneral));
    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_size_request(160, -1);
    let pages = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    pages.set_vexpand(true);
    pages.append(&sidebar);
    pages.append(&stack);

    let ok = gtk::Button::with_label(tr!(BtnOk));
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label(tr!(BtnCancel));
    let apply_button = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnApply)));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&ok);
    buttons.append(&cancel);
    buttons.append(&apply_button);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&pages);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&ok));

    // what is stored now; Apply stores the controls' values over it
    let stored = Rc::new(RefCell::new(options));
    let apply = Rc::new(apply);
    let store = {
        let (window, language, stored, apply) =
            (window.downgrade(), language.downgrade(), stored.clone(), apply.clone());
        move || {
            let (Some(window), Some(language)) = (window.upgrade(), language.upgrade()) else { return };
            let mut new = stored.borrow().clone();
            if let Some((code, _)) = languages.get(language.selected() as usize) {
                new.language.clone_from(code);
            }
            if *stored.borrow() == new {
                return;
            }
            let language_changed = new.language != stored.borrow().language;
            apply(&new);
            *stored.borrow_mut() = new;
            // the windows are built in the language of the start
            if language_changed {
                gtk::AlertDialog::builder()
                    .message(tr!(LangRestart))
                    .modal(true)
                    .build()
                    .show(window.transient_for().as_ref());
            }
        }
    };
    let store = Rc::new(store);
    apply_button.connect_clicked({
        let store = store.clone();
        move |_| store()
    });
    ok.connect_clicked({
        let (store, window) = (store.clone(), window.downgrade());
        move |_| {
            store();
            if let Some(w) = window.upgrade() {
                w.close();
            }
        }
    });
    cancel.connect_clicked({
        let window = window.downgrade();
        move |_| {
            if let Some(w) = window.upgrade() {
                w.close();
            }
        }
    });
    window.present();
}
