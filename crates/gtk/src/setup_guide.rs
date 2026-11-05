//! Options > Setup Guide (D53): the first start's few questions, one page at a time, in a modal
//! window. The pages, what they set and the help texts come from
//! [`rookparcel_frontend::setup_guide`]; the controls are drawn as Preferences draws its fields
//! ([`crate::preferences::Builder`]), a question with two or three answers as option buttons.
//! Skip (or closing the window) keeps everything as it is; Finish applies what was changed.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use rookparcel_frontend::lang::{self, gtk_mnemonic};
use rookparcel_frontend::options::Options;
use rookparcel_frontend::pref_pages::{self, Features, Item, Page};
use rookparcel_frontend::setup_guide::{self, Answers, Guide};
use rookparcel_frontend::tr;

use crate::preferences::{Builder, Ctl};

/// A control of the guide: a Preferences field's, or a question's option buttons with their
/// answers' values.
enum Control {
    Field(Ctl),
    Options(Vec<gtk::CheckButton>, Vec<String>),
}

impl Control {
    fn value(&self) -> String {
        match self {
            Control::Field(c) => c.value(),
            Control::Options(buttons, values) => {
                let i = buttons.iter().position(|b| b.is_active()).unwrap_or(0);
                values.get(i).cloned().unwrap_or_default()
            }
        }
    }

    fn set(&self, value: &str) {
        match self {
            Control::Field(c) => c.set(value),
            Control::Options(buttons, values) => {
                let i = values.iter().position(|v| v == value).unwrap_or(0);
                if let Some(b) = buttons.get(i) {
                    b.set_active(true);
                }
            }
        }
    }

    fn widget(&self) -> Option<gtk::Widget> {
        match self {
            Control::Field(c) => Some(c.widget()),
            Control::Options(buttons, _) => buttons.first().map(|b| b.clone().upcast()),
        }
    }
}

/// Whether Back and Next are enabled on page `page` (from 0) of `count`.
fn nav(page: usize, count: usize) -> (bool, bool) {
    (page > 0, page + 1 < count)
}

/// What the guide offers here: the last page where the desktop integration exists, without the
/// associations where they are the package's or cannot be registered.
pub fn features() -> Features {
    Features {
        desktop: crate::integration::AVAILABLE,
        packaged: crate::integration::packaged() || !crate::integration::associations_settable(),
        ..Features::default()
    }
}

/// What [`show`] calls when the guide closes.
type Done = Box<dyn FnOnce(Option<Options>)>;

struct State {
    guide: Guide,
    controls: HashMap<&'static str, Control>,
    needs: Vec<(gtk::Widget, &'static [&'static str])>,
    stack: gtk::Stack,
    page: Cell<usize>,
    back: gtk::Button,
    next: gtk::Button,
    options: Options,
    /// Called once: the new options on Finish, `None` on Skip or close.
    done: RefCell<Option<Done>>,
}

impl State {
    fn show_page(&self, page: usize) {
        let count = self.guide.pages.len();
        let page = page.min(count.saturating_sub(1));
        self.page.set(page);
        self.stack.set_visible_child_name(&page.to_string());
        let (back, next) = nav(page, count);
        self.back.set_sensitive(back);
        self.next.set_sensitive(next);
    }

    fn update_sensitivity(&self) {
        let on = |k: &str| {
            let Some(ctl) = self.controls.get(k) else { return false };
            let value = ctl.value();
            pref_pages::fields(&self.guide.pages).any(|f| f.key == k && pref_pages::is_on(&f.control, &value))
        };
        for (widget, needs) in &self.needs {
            widget.set_sensitive(needs.iter().all(|k| on(k)));
        }
    }

    fn answers(&self) -> Answers {
        self.controls.iter().map(|(k, c)| (*k, c.value())).collect()
    }

    fn finish(&self, window: &gtk::Window) {
        let mut o = self.options.clone();
        match self.guide.apply(&self.answers(), &mut o) {
            Ok(()) => {
                // taken first: `done` may open windows, and closing this one asks again
                let done = self.done.borrow_mut().take();
                if let Some(done) = done {
                    done(Some(o));
                }
                window.close();
            }
            Err((key, message)) => {
                if let Some(page) = self.guide.page_of(key) {
                    self.show_page(page);
                }
                if let Some(w) = self.controls.get(key).and_then(Control::widget) {
                    w.grab_focus();
                }
                gtk::AlertDialog::builder().message(message).modal(true).build().show(Some(window));
            }
        }
    }
}

/// One page: the step, the title, the page's text, then each field with its help text under it.
fn page_widget(
    b: &mut Builder,
    controls: &mut HashMap<&'static str, Control>,
    page: &Page,
    index: usize,
    count: usize,
    o: &Options,
) -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let step = gtk::Label::new(Some(&setup_guide::step_text(index, count)));
    step.set_xalign(0.0);
    step.add_css_class("dim-label");
    content.append(&step);
    let title = gtk::Label::new(Some(lang::text(page.title)));
    title.set_xalign(0.0);
    title.add_css_class("title-3");
    let attrs = gtk::pango::AttrList::new();
    attrs.insert(gtk::pango::AttrInt::new_weight(gtk::pango::Weight::Bold));
    title.set_attributes(Some(&attrs));
    content.append(&title);
    let text = gtk::Label::new(Some(lang::text(setup_guide::page_text(page))));
    text.set_xalign(0.0);
    text.set_wrap(true);
    text.set_margin_bottom(8);
    content.append(&text);
    let mut last_help = None;
    for item in &page.items {
        match item {
            Item::Field(f) => {
                if let Some(choices) = setup_guide::as_options(&f.control, o) {
                    if let Some(label) = f.label {
                        let l = gtk::Label::new(Some(&lang::plain(lang::text(label))));
                        l.set_xalign(0.0);
                        l.set_wrap(true);
                        content.append(&l);
                    }
                    let mut buttons: Vec<gtk::CheckButton> = Vec::new();
                    for (_, text) in &choices {
                        let r = gtk::CheckButton::with_label(text);
                        r.set_margin_start(12);
                        if let Some(first) = buttons.first() {
                            r.set_group(Some(first));
                        }
                        b.help(&r, f.help_key());
                        content.append(&r);
                        buttons.push(r);
                    }
                    let values = choices.into_iter().map(|(v, _)| v).collect();
                    controls.insert(f.key, Control::Options(buttons, values));
                } else {
                    b.field(&content, f, o);
                    if let Some(c) = b.controls.remove(f.key) {
                        controls.insert(f.key, Control::Field(c));
                    }
                }
                // a field without a label shares the box's text above it: once is enough; a
                // question's answers have the page's note under them (and the text as tooltip)
                let h = f.help_key();
                let h = if setup_guide::as_options(&f.control, o).is_some() { None } else { h };
                if let Some(h) = h.filter(|h| last_help != Some(*h)) {
                    let l = gtk::Label::new(Some(lang::help(h)));
                    l.set_xalign(0.0);
                    l.set_wrap(true);
                    l.set_margin_start(if f.indent { 40 } else { 20 });
                    l.add_css_class("dim-label");
                    content.append(&l);
                }
                last_help = h;
            }
            Item::Note(note) => {
                let l = gtk::Label::new(Some(&note()));
                l.set_xalign(0.0);
                l.set_wrap(true);
                l.add_css_class("dim-label");
                content.append(&l);
            }
            _ => {}
        }
    }
    gtk::ScrolledWindow::builder().child(&content).hscrollbar_policy(gtk::PolicyType::Never).build().upcast()
}

/// Shows the guide for `options` (its `integration` as the system has it), over `parent` or,
/// at a first start, as the application's only window. `done` gets the new options on Finish,
/// `None` on Skip or when the window is closed.
pub fn show(
    app: &gtk::Application,
    parent: Option<&gtk::Window>,
    options: Options,
    done: impl FnOnce(Option<Options>) + 'static,
) {
    let window = gtk::Window::builder()
        .title(tr!(GuideTitle))
        .application(app)
        .modal(parent.is_some())
        .default_width(600)
        .default_height(500)
        .build();
    window.set_transient_for(parent);
    let mut guide = Guide::new(features());
    let count = guide.pages.len();
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_hexpand(true);
    let mut b = Builder::default();
    let mut controls = HashMap::new();
    for (i, page) in guide.pages.iter().enumerate() {
        let w = page_widget(&mut b, &mut controls, page, i, count, &options);
        stack.add_named(&w, Some(&i.to_string()));
    }
    let values = guide.write(&options);

    let skip = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(GuideSkip)));
    let back = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(GuideBack)));
    let next = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(GuideNext)));
    let finish = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(GuideFinish)));
    finish.add_css_class("suggested-action");
    skip.set_hexpand(true);
    skip.set_halign(gtk::Align::Start);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for button in [&skip, &back, &next, &finish] {
        buttons.append(button);
    }
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&stack);
    body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&finish));
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);

    let browse = std::mem::take(&mut b.browse);
    let needs = std::mem::take(&mut b.needs);
    let st = Rc::new(State {
        guide,
        controls,
        needs,
        stack,
        page: Cell::new(0),
        back: back.clone(),
        next: next.clone(),
        options,
        done: RefCell::new(Some(Box::new(done))),
    });
    for (key, value) in values {
        if let Some(c) = st.controls.get(key) {
            c.set(&value);
        }
    }
    // the boxes grey what depends on them
    for ctl in st.controls.values() {
        if let Control::Field(Ctl::Check(c)) = ctl {
            let w = Rc::downgrade(&st);
            c.connect_toggled(move |_| {
                if let Some(st) = w.upgrade() {
                    st.update_sensitivity();
                }
            });
        }
    }
    for (button, key, kind) in browse {
        let (w, win) = (Rc::downgrade(&st), window.downgrade());
        button.connect_clicked(move |_| {
            let (Some(st), Some(win)) = (w.upgrade(), win.upgrade()) else { return };
            if let Some(Control::Field(Ctl::Entry(entry))) = st.controls.get(key) {
                crate::preferences::pick(&win, entry, kind);
            }
        });
    }
    back.connect_clicked({
        let w = Rc::downgrade(&st);
        move |_| {
            if let Some(st) = w.upgrade() {
                st.show_page(st.page.get().saturating_sub(1));
            }
        }
    });
    next.connect_clicked({
        let w = Rc::downgrade(&st);
        move |_| {
            if let Some(st) = w.upgrade() {
                st.show_page(st.page.get() + 1);
            }
        }
    });
    finish.connect_clicked({
        let (w, win) = (Rc::downgrade(&st), window.downgrade());
        move |_| {
            if let (Some(st), Some(win)) = (w.upgrade(), win.upgrade()) {
                st.finish(&win);
            }
        }
    });
    skip.connect_clicked({
        let win = window.downgrade();
        move |_| {
            if let Some(win) = win.upgrade() {
                win.close();
            }
        }
    });
    window.connect_close_request({
        let st = st.clone();
        move |_| {
            // Skip, Escape or the title bar's close: nothing applied
            let done = st.done.borrow_mut().take();
            if let Some(done) = done {
                done(None);
            }
            glib::Propagation::Proceed
        }
    });
    st.update_sensitivity();
    st.show_page(0);
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_next_follow_the_page() {
        assert_eq!(nav(0, 4), (false, true));
        assert_eq!(nav(1, 4), (true, true));
        assert_eq!(nav(3, 4), (true, false));
        assert_eq!(nav(0, 1), (false, false));
    }
}
