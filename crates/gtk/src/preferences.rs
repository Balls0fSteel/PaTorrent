//! Options > Preferences: µTorrent 2.2.1's dialog, the page list on the left and the page on the
//! right, OK / Cancel / Apply, built from the frontend's description of the pages
//! ([`pref_pages`]): every page the Win32 dialog has but the Web UI (the remote mode); General
//! has the desktop integration in place of the Windows one, and UI Settings the tray boxes
//! while a tray is on screen.
//!
//! The pages and the Advanced table edit one working [`Options`]. The controls are written from
//! it ([`Form::write`]) and only the ones the user changed are read back ([`Form::read`]), on
//! Apply, on OK and when the Advanced table opens (it shows what the pages say).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_frontend::lang::{self, S, gtk_mnemonic};
use patorrent_frontend::options::{self, Options};
use patorrent_frontend::pref_pages::{
    self, Action, Control, Features, Field, Form, Item, Page, SCHED_ENABLE, TextKind, advanced_name,
    advanced_rows,
};
use patorrent_frontend::scheduler::{CELLS, Mode};
use patorrent_frontend::settings::{Entry, Kind, Value};
use patorrent_frontend::tr;

use crate::sched_grid::SchedGrid;
use crate::table::Table;

/// Loads a block list: the number of rules or an error.
pub type ReloadIpFilter = Box<dyn Fn(&Path) -> Result<usize, String>>;

/// What the dialog asks of its owner.
pub struct Hooks {
    /// OK and Apply: store the options and put them into effect.
    pub apply: Box<dyn Fn(&Options)>,
    /// "Reload" on the BitTorrent page: load this block list; the number of rules or an error.
    pub reload_ip_filter: ReloadIpFilter,
    /// Where `ipfilter.dat` is looked for when no path is set.
    pub settings_dir: PathBuf,
    /// A tray is on screen: UI Settings shows the tray and notification boxes.
    pub tray: bool,
}

/// A field's control.
enum Ctl {
    Check(gtk::CheckButton),
    Entry(gtk::Entry),
    /// The drop-down and its choices' values.
    Choice(gtk::DropDown, Vec<String>),
}

impl Ctl {
    fn value(&self) -> String {
        match self {
            Ctl::Check(c) => c.is_active().to_string(),
            Ctl::Entry(e) => e.text().to_string(),
            Ctl::Choice(d, values) => values.get(d.selected() as usize).cloned().unwrap_or_default(),
        }
    }

    fn set(&self, value: &str) {
        match self {
            Ctl::Check(c) => c.set_active(value == "true"),
            Ctl::Entry(e) => e.set_text(value),
            Ctl::Choice(d, values) => {
                d.set_selected(values.iter().position(|v| v == value).unwrap_or(0) as u32)
            }
        }
    }

    fn widget(&self) -> gtk::Widget {
        match self {
            Ctl::Check(c) => c.clone().upcast(),
            Ctl::Entry(e) => e.clone().upcast(),
            Ctl::Choice(d, _) => d.clone().upcast(),
        }
    }
}

/// The Advanced page's table and value controls.
struct Advanced {
    filter: gtk::Entry,
    table: Table,
    value: gtk::Entry,
    yes: gtk::CheckButton,
    no: gtk::CheckButton,
    set: gtk::Button,
    reset: gtk::Button,
}

/// What the handlers share. It holds no window, so the widgets' closures can keep it without
/// a cycle (they hold it weakly); the window's close handler keeps it alive.
struct Dialog {
    pages: Vec<Page>,
    /// The working copy every page edits.
    opts: RefCell<Options>,
    form: RefCell<Form>,
    controls: HashMap<&'static str, Ctl>,
    /// Rows greyed unless the check boxes named are on.
    needs: Vec<(gtk::Widget, &'static [&'static str])>,
    stack: gtk::Stack,
    /// The page on screen, to go back to when the pages cannot be read.
    page: RefCell<String>,
    ip_status: Option<gtk::Label>,
    advanced: Option<Advanced>,
    /// The Scheduler page's table and what it showed when written, so an untouched table keeps
    /// the stored one.
    sched: Option<SchedGrid>,
    sched_shown: RefCell<[Mode; CELLS]>,
    /// The Advanced table's rows.
    rows: RefCell<Vec<&'static Entry>>,
    hooks: Hooks,
}

/// Shows the dialog over `parent`, editing `options` as stored.
pub fn show(parent: &gtk::ApplicationWindow, options: Options, hooks: Hooks) {
    let window = gtk::Window::builder()
        .title(tr!(PrefTitle))
        .transient_for(parent)
        .modal(true)
        .default_width(760)
        .default_height(560)
        .build();
    let mut pages = pref_pages::pages(Features {
        desktop: crate::integration::AVAILABLE,
        packaged: crate::integration::packaged(),
        tray: hooks.tray,
        // a Flatpak is updated by Flathub
        updates: patorrent_frontend::update::AVAILABLE && !crate::integration::packaged(),
        // GTK's lists have no alternate shading yet (BACKLOG, GTK desktop checks)
        alternate_rows: false,
    });
    if !crate::integration::associations_settable() {
        // macOS outside PaTorrent.app: no bundle id to register, autostart still works
        fix_fields(&mut pages, &["assoc.torrent", "assoc.magnet", "ui.check_assoc"]);
    }
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    let mut b = Builder::default();
    for page in &pages {
        let child = b.page(page, &options);
        let title = match page.parent {
            // one level down, as under Advanced in µTorrent's tree
            Some(_) => format!("    {}", lang::text(page.title)),
            None => lang::text(page.title).to_owned(),
        };
        stack.add_titled(&child, Some(page.key), &title);
    }
    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_size_request(170, -1);
    let body_pages = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    body_pages.set_vexpand(true);
    body_pages.append(&sidebar);
    body_pages.append(&stack);

    let ok = gtk::Button::with_label(tr!(BtnOk));
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label(tr!(BtnCancel));
    let apply = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnApply)));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&ok);
    buttons.append(&cancel);
    buttons.append(&apply);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&body_pages);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&ok));
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);

    let Builder { controls, needs, ip_status, advanced, sched, browse, actions, .. } = b;
    let d = Rc::new(Dialog {
        page: RefCell::new(pages.first().map(|p| p.key.to_owned()).unwrap_or_default()),
        pages,
        opts: RefCell::new(options),
        form: RefCell::default(),
        controls,
        needs,
        stack: stack.clone(),
        ip_status,
        advanced,
        sched,
        sched_shown: RefCell::new([Mode::Full; CELLS]),
        rows: RefCell::default(),
        hooks,
    });
    DIALOG.with(|x| *x.borrow_mut() = Some(Rc::downgrade(&d)));

    // the check boxes and the proxy type grey what depends on them
    for ctl in d.controls.values() {
        let w = Rc::downgrade(&d);
        let update = move || {
            if let Some(d) = w.upgrade() {
                d.update_sensitivity();
            }
        };
        match ctl {
            Ctl::Check(c) => {
                c.connect_toggled(move |_| update());
            }
            Ctl::Choice(dd, _) => {
                dd.connect_selected_notify(move |_| update());
            }
            Ctl::Entry(_) => {}
        }
    }
    // "..." beside folders and files
    for (button, key, kind) in browse {
        let (w, window) = (Rc::downgrade(&d), window.downgrade());
        button.connect_clicked(move |_| {
            let (Some(d), Some(window)) = (w.upgrade(), window.upgrade()) else { return };
            let Some(Ctl::Entry(entry)) = d.controls.get(key) else { return };
            pick(&window, entry, kind);
        });
    }
    for (button, action) in actions {
        let w = Rc::downgrade(&d);
        button.connect_clicked(move |_| {
            if let Some(d) = w.upgrade() {
                d.act(action);
            }
        });
    }
    if let Some(a) = &d.advanced {
        let w = Rc::downgrade(&d);
        a.filter.connect_changed(move |_| {
            if let Some(d) = w.upgrade() {
                d.fill_advanced(None);
            }
        });
        let w = Rc::downgrade(&d);
        a.table.selection.connect_selection_changed(move |_, _, _| {
            if let Some(d) = w.upgrade() {
                d.show_value();
            }
        });
        for (button, reset) in [(&a.set, false), (&a.reset, true)] {
            let w = Rc::downgrade(&d);
            button.connect_clicked(move |_| {
                if let Some(d) = w.upgrade() {
                    d.advanced_set(reset);
                }
            });
        }
        // Enter in the value sets it instead of closing the dialog
        let w = Rc::downgrade(&d);
        a.value.connect_activate(move |_| {
            if let Some(d) = w.upgrade() {
                d.advanced_set(false);
            }
        });
    }
    // the Advanced table shows what the pages say: they are read when it opens
    stack.connect_visible_child_name_notify({
        let w = Rc::downgrade(&d);
        move |stack| {
            let Some(d) = w.upgrade() else { return };
            let Some(name) = stack.visible_child_name() else { return };
            if *d.page.borrow() == name.as_str() {
                return;
            }
            if name == "advanced" && !d.read_or_complain() {
                return;
            }
            if name == "advanced" {
                d.fill_advanced(None);
            }
            *d.page.borrow_mut() = name.to_string();
        }
    });

    apply.connect_clicked({
        let (w, window) = (Rc::downgrade(&d), window.downgrade());
        move |_| {
            if let (Some(d), Some(window)) = (w.upgrade(), window.upgrade()) {
                d.apply(&window);
            }
        }
    });
    ok.connect_clicked({
        let (w, window) = (Rc::downgrade(&d), window.downgrade());
        move |_| {
            let (Some(d), Some(window)) = (w.upgrade(), window.upgrade()) else { return };
            if d.apply(&window) {
                window.close();
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
    window.connect_close_request({
        let d = d.clone();
        move |_| {
            // the shared state goes with the window
            let _ = &d;
            glib::Propagation::Proceed
        }
    });

    d.write_controls();
    d.update_ip_status(None);
    window.present();
}

impl Dialog {
    /// The controls from the working options.
    fn write_controls(&self) {
        let values = self.form.borrow_mut().write(&self.pages, &self.opts.borrow());
        for (key, value) in values {
            if let Some(c) = self.controls.get(key) {
                c.set(&value);
            }
        }
        if let Some(g) = &self.sched {
            let table = self.opts.borrow().gui.schedule.table;
            g.set(&table);
            *self.sched_shown.borrow_mut() = table;
        }
        self.update_sensitivity();
    }

    fn update_sensitivity(&self) {
        let on = |k: &str| {
            let Some(ctl) = self.controls.get(k) else { return false };
            let value = ctl.value();
            pref_pages::fields(&self.pages).any(|f| f.key == k && pref_pages::is_on(&f.control, &value))
        };
        for (widget, needs) in &self.needs {
            widget.set_sensitive(needs.iter().all(|k| on(k)));
        }
    }

    /// Reads the changed controls into the working options; on a bad value shows its page,
    /// says what is wrong and focuses the control.
    fn read_or_complain(&self) -> bool {
        let read = {
            let form = self.form.borrow();
            let opts = self.opts.borrow();
            form.read(&self.pages, &opts, |k| self.controls.get(k).map(Ctl::value).unwrap_or_default())
        };
        match read {
            Ok(mut o) => {
                if let Some(g) = &self.sched {
                    let table = g.table();
                    if table != *self.sched_shown.borrow() {
                        o.gui.schedule.table = table;
                    }
                }
                *self.opts.borrow_mut() = o;
                true
            }
            Err((key, message)) => {
                if let Some(page) = pref_pages::page_of(&self.pages, key) {
                    *self.page.borrow_mut() = page.key.to_owned();
                    self.stack.set_visible_child_name(page.key);
                }
                let parent = self.stack.root().and_downcast::<gtk::Window>();
                gtk::AlertDialog::builder().message(message).modal(true).build().show(parent.as_ref());
                if let Some(c) = self.controls.get(key) {
                    c.widget().grab_focus();
                }
                false
            }
        }
    }

    /// OK and Apply: false when a value is wrong.
    fn apply(&self, window: &gtk::Window) -> bool {
        let language = self.opts.borrow().gui.language.clone();
        if !self.read_or_complain() {
            return false;
        }
        (self.hooks.apply)(&self.opts.borrow());
        // the windows are built in the language of the start
        if self.opts.borrow().gui.language != language {
            gtk::AlertDialog::builder()
                .message(tr!(LangRestart))
                .modal(true)
                .build()
                .show(window.transient_for().as_ref());
        }
        self.write_controls();
        self.fill_advanced(self.selected_row());
        true
    }

    fn act(&self, action: Action) {
        match action {
            Action::RandomPort => {
                if let Some(c) = self.controls.get("port") {
                    c.set(&options::random_port().to_string());
                }
            }
            Action::ReloadIpFilter => {
                let on = matches!(self.controls.get("ipfilter.enable"), Some(Ctl::Check(c)) if c.is_active());
                if !on {
                    self.update_ip_status(Some(Err(tr!(PrefIpFilterOff).to_owned())));
                    return;
                }
                let mut gui = self.opts.borrow().gui.clone();
                gui.ip_filter_path = self.controls.get("ipfilter.path").map(Ctl::value).unwrap_or_default();
                gui.ip_filter_path = gui.ip_filter_path.trim().to_owned();
                let file = gui.ip_filter_file(&self.hooks.settings_dir);
                let result =
                    (self.hooks.reload_ip_filter)(&file).map_err(|e| format!("{}: {e}", file.display()));
                self.update_ip_status(Some(result));
            }
        }
    }

    fn update_ip_status(&self, result: Option<Result<usize, String>>) {
        let Some(label) = &self.ip_status else { return };
        let path = self.controls.get("ipfilter.path").map(Ctl::value).unwrap_or_default();
        let text = match result {
            Some(Ok(n)) => tr!(PrefIpFilterLoaded, count = n),
            Some(Err(e)) => e,
            None if path.trim().is_empty() => {
                tr!(PrefIpFilterDefault, path = self.hooks.settings_dir.join("ipfilter.dat").display())
            }
            None => String::new(),
        };
        label.set_text(&text);
    }

    fn selected_row(&self) -> Option<usize> {
        self.advanced.as_ref()?.table.selected().first().copied()
    }

    /// Fills the table with the rows matching the filter; selects `select` (a row) again.
    fn fill_advanced(&self, select: Option<usize>) {
        let Some(a) = &self.advanced else { return };
        let keep = select.and_then(|i| self.rows.borrow().get(i).map(|e| e.name));
        *self.rows.borrow_mut() = advanced_rows(&a.filter.text());
        let n = self.rows.borrow().len();
        // a new filter can keep the count, so the rows are redrawn either way
        a.table.update(n);
        a.table.unselect_all();
        if let Some(i) = keep.and_then(|name| self.rows.borrow().iter().position(|e| e.name == name)) {
            a.table.selection.select_item(i as u32, true);
            a.table.view.scroll_to(i as u32, None, gtk::ListScrollFlags::NONE, None);
        }
        self.show_value();
    }

    /// The value controls for the selected row: True / False for switches, a field otherwise.
    fn show_value(&self) {
        let Some(a) = &self.advanced else { return };
        let entry = self.selected_row().and_then(|i| self.rows.borrow().get(i).copied());
        let is_bool = entry.is_some_and(|e| e.kind == Kind::Bool);
        a.value.set_visible(!is_bool);
        a.yes.set_visible(is_bool);
        a.no.set_visible(is_bool);
        match entry.map(|e| e.get(&self.opts.borrow().settings)) {
            Some(Value::Bool(v)) => {
                a.yes.set_active(v);
                a.no.set_active(!v);
            }
            Some(v) => a.value.set_text(&v.to_string()),
            None => a.value.set_text(""),
        }
        for w in [a.value.upcast_ref::<gtk::Widget>(), a.set.upcast_ref(), a.reset.upcast_ref()] {
            w.set_sensitive(entry.is_some());
        }
    }

    /// "Set" (or "Reset" to the default) for the selected row; the pages follow.
    fn advanced_set(&self, reset: bool) {
        let Some(a) = &self.advanced else { return };
        let Some(i) = self.selected_row() else { return };
        let Some(e) = self.rows.borrow().get(i).copied() else { return };
        let text = if reset {
            e.get(&patorrent_core::Settings::default()).to_string()
        } else if e.kind == Kind::Bool {
            a.yes.is_active().to_string()
        } else {
            a.value.text().to_string()
        };
        let result = e.set(&mut self.opts.borrow_mut().settings, &text);
        if let Err(message) = result {
            let parent = self.stack.root().and_downcast::<gtk::Window>();
            gtk::AlertDialog::builder().message(message).modal(true).build().show(parent.as_ref());
            a.value.grab_focus();
            return;
        }
        // the pages show the table's values (they were read when Advanced opened)
        self.write_controls();
        self.fill_advanced(Some(i));
    }

    /// A cell of the Advanced table.
    fn cell(&self, row: usize, column: usize) -> String {
        let Some(e) = self.rows.borrow().get(row).copied() else { return String::new() };
        let opts = self.opts.borrow();
        match column {
            0 => advanced_name(e, &opts.settings),
            _ => e.get(&opts.settings).to_string(),
        }
    }
}

/// Greys the fields `keys` ([`Field::fixed`]): shown, never edited.
fn fix_fields(pages: &mut [Page], keys: &[&str]) {
    for page in pages {
        for item in &mut page.items {
            if let Item::Field(f) = item {
                if keys.contains(&f.key) {
                    f.fixed = true;
                }
            }
        }
    }
}

/// A folder or file picker for `entry`.
fn pick(window: &gtk::Window, entry: &gtk::Entry, kind: TextKind) {
    let (title, folder) = match kind {
        TextKind::Folder(t) => (t, true),
        TextKind::File(t) => (t, false),
        _ => return,
    };
    let dialog = gtk::FileDialog::builder().title(lang::text(title)).modal(true).build();
    let now = entry.text();
    let now = Path::new(now.trim());
    if folder && now.is_dir() {
        dialog.set_initial_folder(Some(&gio::File::for_path(now)));
    } else if !folder && now.is_file() {
        dialog.set_initial_file(Some(&gio::File::for_path(now)));
    }
    let entry = entry.downgrade();
    let done = move |r: Result<gio::File, glib::Error>| {
        if let (Some(path), Some(entry)) = (r.ok().and_then(|f| f.path()), entry.upgrade()) {
            entry.set_text(&path.to_string_lossy());
        }
    };
    if folder {
        dialog.select_folder(Some(window), None::<&gio::Cancellable>, done);
    } else {
        dialog.open(Some(window), None::<&gio::Cancellable>, done);
    }
}

/// Builds the pages' widgets and collects what the dialog needs from them.
#[derive(Default)]
struct Builder {
    controls: HashMap<&'static str, Ctl>,
    needs: Vec<(gtk::Widget, &'static [&'static str])>,
    ip_status: Option<gtk::Label>,
    advanced: Option<Advanced>,
    sched: Option<SchedGrid>,
    /// "..." buttons: the button, its field and the picker.
    browse: Vec<(gtk::Button, &'static str, TextKind)>,
    actions: Vec<(gtk::Button, Action)>,
    /// The row the next `beside` field or button joins.
    last_row: Option<gtk::Box>,
}

impl Builder {
    fn page(&mut self, page: &Page, o: &Options) -> gtk::Widget {
        self.last_row = None;
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_end(6);
        // where fields go: the page, or the group box opened last
        let mut target = content.clone();
        for item in &page.items {
            match item {
                Item::Group(title) => {
                    let inner = gtk::Box::new(gtk::Orientation::Vertical, 6);
                    inner.set_margin_top(6);
                    inner.set_margin_bottom(8);
                    inner.set_margin_start(8);
                    inner.set_margin_end(8);
                    let frame = gtk::Frame::new(Some(&lang::plain(lang::text(*title))));
                    frame.set_child(Some(&inner));
                    content.append(&frame);
                    target = inner;
                }
                Item::Field(f) => self.field(&target, f, o),
                Item::Note(text) => {
                    let l = gtk::Label::new(Some(&text()));
                    l.set_xalign(0.0);
                    l.set_wrap(true);
                    l.add_css_class("dim-label");
                    target.append(&l);
                }
                Item::Button { action, label } => {
                    let b = gtk::Button::with_mnemonic(&gtk_mnemonic(lang::text(*label)));
                    match &self.last_row {
                        Some(row) => row.append(&b),
                        None => target.append(&b),
                    }
                    self.actions.push((b, *action));
                }
                Item::IpFilterStatus => {
                    let l = gtk::Label::new(None);
                    l.set_xalign(0.0);
                    l.set_wrap(true);
                    target.append(&l);
                    self.ip_status = Some(l);
                }
                Item::AdvancedTable => {
                    let (widget, advanced) = advanced_page();
                    target.append(&widget);
                    self.advanced = Some(advanced);
                }
                Item::SchedTable => {
                    let grid = SchedGrid::new();
                    target.append(&grid.widget);
                    self.needs.push((grid.widget.clone().upcast(), &[SCHED_ENABLE]));
                    self.sched = Some(grid);
                }
            }
        }
        if page.items.iter().any(|i| matches!(i, Item::AdvancedTable)) {
            // the table scrolls itself
            return content.upcast();
        }
        gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build()
            .upcast()
    }

    fn field(&mut self, target: &gtk::Box, f: &Field, o: &Options) {
        let caption = |text: S| {
            let l = gtk::Label::with_mnemonic(&gtk_mnemonic(lang::text(text)));
            l.set_xalign(0.0);
            l.set_wrap(true);
            l.set_hexpand(true);
            l
        };
        let (ctl, grey): (Ctl, gtk::Widget) = match f.control {
            Control::Check { .. } => {
                let c = gtk::CheckButton::with_mnemonic(&gtk_mnemonic(f.label.map_or("", lang::text)));
                c.set_hexpand(true);
                (Ctl::Check(c.clone()), c.upcast())
            }
            Control::Number { .. } | Control::Text { .. } => {
                let e = gtk::Entry::new();
                e.set_activates_default(true);
                match f.control {
                    Control::Number { .. } => {
                        e.set_width_chars(8);
                        e.set_max_width_chars(10);
                        e.set_input_purpose(gtk::InputPurpose::Number);
                    }
                    Control::Text { kind, .. } => {
                        e.set_hexpand(true);
                        if kind == TextKind::Password {
                            e.set_visibility(false);
                            e.set_input_purpose(gtk::InputPurpose::Password);
                        }
                    }
                    _ => {}
                }
                (Ctl::Entry(e.clone()), e.upcast())
            }
            Control::Choice { choices, .. } => {
                let list = choices(o);
                let texts: Vec<&str> = list.iter().map(|(_, t)| t.as_str()).collect();
                let d = gtk::DropDown::from_strings(&texts);
                (Ctl::Choice(d.clone(), list.into_iter().map(|(v, _)| v).collect()), d.upcast())
            }
        };
        let control = ctl.widget();
        if f.beside {
            if let Some(row) = &self.last_row {
                row.append(&control);
                if f.fixed {
                    grey.set_sensitive(false);
                } else if !f.needs.is_empty() {
                    self.needs.push((grey, f.needs));
                }
                self.controls.insert(f.key, ctl);
                return;
            }
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if f.indent {
            row.set_margin_start(20);
        }
        if let (Some(label), false) = (f.label, matches!(f.control, Control::Check { .. })) {
            let l = caption(label);
            l.set_mnemonic_widget(Some(&control));
            row.append(&l);
        }
        row.append(&control);
        if let Control::Text { kind: kind @ (TextKind::Folder(_) | TextKind::File(_)), .. } = f.control {
            let b = gtk::Button::with_label("...");
            row.append(&b);
            self.browse.push((b, f.key, kind));
        }
        target.append(&row);
        if f.fixed {
            // what a package manages: shown, never edited
            row.set_sensitive(false);
        } else if !f.needs.is_empty() {
            self.needs.push((row.clone().upcast(), f.needs));
        }
        self.last_row = Some(row);
        self.controls.insert(f.key, ctl);
    }
}

/// The Advanced page: Filter, the table of every engine setting, the value with Set and Reset.
fn advanced_page() -> (gtk::Widget, Advanced) {
    let filter = gtk::Entry::new();
    filter.set_hexpand(true);
    let filter_label = gtk::Label::with_mnemonic(&gtk_mnemonic(tr!(PrefFilter)));
    filter_label.set_mnemonic_widget(Some(&filter));
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&filter_label);
    top.append(&filter);

    let text: crate::table::CellText = Rc::new(|row, column| {
        DIALOG
            .with(|d| d.borrow().as_ref().and_then(|w| w.upgrade()).map(|d| d.cell(row, column)))
            .unwrap_or_default()
    });
    let table = Table::new(&[(S::ColName, 300, false), (S::ColValue, 140, false)], text, None, None);
    let scroll = gtk::ScrolledWindow::builder().child(&table.view).vexpand(true).has_frame(true).build();

    let value = gtk::Entry::new();
    value.set_hexpand(true);
    let yes = gtk::CheckButton::with_label(tr!(PrefTrue));
    let no = gtk::CheckButton::with_label(tr!(PrefFalse));
    no.set_group(Some(&yes));
    let set = gtk::Button::with_label(tr!(PrefSet));
    let reset = gtk::Button::with_label(tr!(PrefReset));
    let value_label = gtk::Label::new(Some(tr!(PrefValue)));
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    for w in [
        value_label.upcast_ref::<gtk::Widget>(),
        value.upcast_ref(),
        yes.upcast_ref(),
        no.upcast_ref(),
        set.upcast_ref(),
        reset.upcast_ref(),
    ] {
        bottom.append(w);
    }
    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.set_vexpand(true);
    page.append(&top);
    page.append(&scroll);
    page.append(&bottom);
    (page.upcast(), Advanced { filter, table, value, yes, no, set, reset })
}

thread_local! {
    /// The open dialog, for the Advanced table's cells (the table is built before the dialog's
    /// shared state exists).
    static DIALOG: RefCell<Option<std::rc::Weak<Dialog>>> = const { RefCell::new(None) };
}
