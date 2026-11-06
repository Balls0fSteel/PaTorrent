//! The Search Sources window (GTK's and Win32's): the sources (Name, the Torznab URL with the key
//! masked), "Search this source" for the selected one, Add, Edit (a double click or Return too),
//! Remove (asked first; ⌫ too) and Test (the source's caps on a thread: "OK, N categories" or the
//! error, the key taken out); under the list the search helper's buttons (D51,
//! `crate::search_helper`) and a status line. Edits are stored at once (`search::store`).
//!
//! Add and Edit open the Add / Edit Search Source sheet: name, Torznab URL with a hint, API key
//! (left empty, the stored key stays; it is never shown) and "Search this source"; a URL that is
//! not `http(s)://` is refused with a message.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSButton, NSColor, NSFont, NSLayoutConstraintOrientation, NSSecureTextField, NSTextField, NSView,
    NSWindow,
};
use objc2_foundation::NSSize;
use rookparcel_frontend::search::view::{SourceForm, apply_form};
use rookparcel_frontend::search::{Source, fetch_caps, redact};
use rookparcel_frontend::tr;

use crate::app::peek;
use crate::dialogs::{self, NOTHING, Observer, cmd};
use crate::search_helper::Buttons;
use crate::table::{self, CellValue, ColumnSpec, Hooks, Key, RPTableSource, RPTableView, TableKind};
use crate::ui::{self, layout, ns};

const ADD: u8 = 1;
const EDIT: u8 = 2;
const REMOVE: u8 = 3;
const TEST: u8 = 4;
const CLOSE: u8 = 5;
/// The window closed.
const CLOSED: u8 = 6;
/// "Search this source" clicked.
const ENABLED: u8 = 7;

/// The editor's OK and Cancel.
pub const EDITOR_OK: u8 = 1;
pub const EDITOR_CANCEL: u8 = 2;

pub struct Manager {
    id: u16,
    pub window: Retained<NSWindow>,
    pub table: Retained<RPTableView>,
    pub rows: Retained<RPTableSource>,
    status: Retained<NSTextField>,
    edit: Retained<NSButton>,
    remove: Retained<NSButton>,
    test: Retained<NSButton>,
    enabled: Retained<NSButton>,
    /// The search helper's buttons (D51).
    pub helper: Buttons,
    observer: RefCell<Option<Observer>>,
}

/// The Add / Edit Search Source sheet.
pub struct Editor {
    pub id: u16,
    pub window: Retained<NSWindow>,
    pub name: Retained<NSTextField>,
    pub url: Retained<NSTextField>,
    pub key: Retained<NSSecureTextField>,
    pub enabled: Retained<NSButton>,
    /// The source edited; `None` adds one.
    index: Option<usize>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Manager>>> = const { RefCell::new(None) };
    static EDITOR: RefCell<Option<Rc<Editor>>> = const { RefCell::new(None) };
    /// The list's sources as shown (keys included: never shown).
    static LIST: RefCell<Vec<Source>> = const { RefCell::new(Vec::new()) };
}

/// The open window.
pub fn open_window() -> Option<Rc<Manager>> {
    OPEN.with(|o| o.borrow().clone())
}

/// The open editor sheet.
pub fn editor() -> Option<Rc<Editor>> {
    EDITOR.with(|o| o.borrow().clone())
}

fn hooks() -> Hooks {
    let mut h = Hooks::flat(
        || LIST.with(|l| l.borrow().len()),
        |row, col, _dark| {
            LIST.with(|l| {
                let l = l.borrow();
                let Some(s) = l.get(row as usize) else { return CellValue::Empty };
                // never the key: the URL as an error would show it
                CellValue::Text(if col == 0 { s.label() } else { redact(&s.url, &s.api_key) })
            })
        },
    );
    h.selection = Some(Box::new(|_| {
        if let Some(m) = open_window() {
            m.buttons();
        }
    }));
    h.activate = Some(Box::new(|row| {
        if let Some(m) = open_window() {
            m.open_editor(Some(row as usize));
        }
    }));
    h.key = Some(Box::new(|key, ids| {
        let (Some(m), Some(&row)) = (open_window(), ids.first()) else { return false };
        match key {
            Key::Return => m.open_editor(Some(row as usize)),
            Key::Delete => m.ask_remove(),
        }
        true
    }));
    h
}

/// Options > Manage Sources (the search field's menu, the Search Results window): the window,
/// brought forward when it is open.
pub fn open() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if let Some(m) = open_window() {
        m.window.makeKeyAndOrderFront(None);
        return;
    }
    let m = build(mtm);
    m.fill(Some(0));
    m.helper.show();
    m.window.makeKeyAndOrderFront(None);
}

fn build(mtm: MainThreadMarker) -> Rc<Manager> {
    let id = dialogs::new_id();
    let specs = vec![
        ColumnSpec { hideable: false, ..ColumnSpec::text("name", &ui::text(tr!(ColName)), 180.0) },
        ColumnSpec::text("url", &ui::text(tr!(ColUrl)), 340.0),
    ];
    let (scroll, table, rows) = table::table(TableKind("search sources"), specs, hooks(), None, mtm);
    table.setAllowsMultipleSelection(false);
    layout::min_width(&scroll, 520.0);
    layout::activate(&[scroll.heightAnchor().constraintGreaterThanOrEqualToConstant(160.0)]);
    scroll.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    let enabled = ui::check(&ui::text(tr!(SearchSourceEnabled)), false, cmd(id, ENABLED), mtm);
    let helper = Buttons::new(id, mtm);
    let status = ui::wrapping_label("", mtm);
    status.setSelectable(true);
    let add = ui::button_tr(tr!(RssFeedsAdd), cmd(id, ADD), mtm);
    let edit = ui::button_tr(tr!(RssFeedsEdit), cmd(id, EDIT), mtm);
    let remove = ui::button_tr(tr!(RssFeedsRemove), cmd(id, REMOVE), mtm);
    let test = ui::button_tr(tr!(SearchTest), cmd(id, TEST), mtm);
    let close = ui::button_tr(tr!(BtnClose), cmd(id, CLOSE), mtm);
    close.setKeyEquivalent(&ns("\u{1b}"));
    let spacer = layout::spacer(mtm);
    let buttons = layout::hstack(&[&add, &edit, &remove, &test, &spacer, &close], mtm);
    let views: [&NSView; 5] = [&scroll, &enabled, &helper.row, &status, &buttons];
    let root = dialogs::column(&views, mtm);
    root.setDistribution(objc2_app_kit::NSStackViewDistribution::Fill);
    let window = dialogs::window(
        &ui::text(tr!(SearchSourcesTitle)),
        &root,
        Some(NSSize::new(600.0, 400.0)),
        true,
        mtm,
    );
    let m = Rc::new(Manager {
        id,
        window: window.clone(),
        table,
        rows,
        status,
        edit,
        remove,
        test,
        enabled,
        helper,
        observer: RefCell::new(None),
    });
    *m.observer.borrow_mut() = Some(dialogs::on_close(&window, id, CLOSED));
    let handler = m.clone();
    dialogs::set_handler(id, move |act| handler.act(act));
    OPEN.with(|o| *o.borrow_mut() = Some(m.clone()));
    window.center();
    m
}

impl Manager {
    fn act(self: &Rc<Self>, act: u8) {
        match act {
            ADD => self.open_editor(None),
            EDIT => self.open_editor(self.selected()),
            REMOVE => self.ask_remove(),
            TEST => self.run_test(),
            CLOSE => self.window.close(),
            CLOSED => self.closed(),
            ENABLED => self.toggle_enabled(),
            a if a >= crate::search_helper::FIRST_ACT => crate::search_helper::act(self, a),
            _ => {}
        }
    }

    fn closed(&self) {
        if let Some(o) = self.observer.borrow_mut().take() {
            dialogs::forget(&o);
        }
        OPEN.with(|o| o.borrow_mut().take());
        dialogs::retire(self.id);
    }

    pub fn selected(&self) -> Option<usize> {
        let n = LIST.with(|l| l.borrow().len());
        self.rows.selected().first().map(|&r| r as usize).filter(|&r| r < n)
    }

    fn buttons(&self) {
        let sel = self.selected();
        for b in [&self.edit, &self.remove, &self.test, &self.enabled] {
            b.setEnabled(sel.is_some());
        }
        let on = sel.and_then(|i| LIST.with(|l| l.borrow().get(i).map(|s| s.enabled))).unwrap_or(false);
        ui::set_checked(&self.enabled, on);
    }

    /// The rows again from the stored sources, `select` picked.
    pub fn fill(&self, select: Option<usize>) {
        LIST.with(|l| *l.borrow_mut() = super::stored_sources());
        let n = LIST.with(|l| l.borrow().len());
        self.rows.reload_keep_selection();
        let ids: Vec<u64> = select.filter(|&i| i < n).map(|i| i as u64).into_iter().collect();
        self.rows.select(&ids);
        self.buttons();
    }

    /// The status line (red for a failure).
    pub fn say(&self, text: &str, failed: bool) {
        self.status.setStringValue(&ns(text));
        let color = if failed { NSColor::systemRedColor() } else { NSColor::labelColor() };
        self.status.setTextColor(Some(&color));
    }

    fn toggle_enabled(&self) {
        let Some(i) = self.selected() else { return };
        let mut sources = super::stored_sources();
        let Some(s) = sources.get_mut(i) else { return };
        s.enabled = ui::checked(&self.enabled);
        super::store(&sources);
        self.fill(Some(i));
    }

    /// Add (`index` `None`) or Edit: the editor sheet.
    pub fn open_editor(self: &Rc<Self>, index: Option<usize>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        if editor().is_some() {
            return;
        }
        let sources = super::stored_sources();
        let current = index.and_then(|i| sources.get(i));
        if index.is_some() && current.is_none() {
            return;
        }
        let e = build_editor(current, index, mtm);
        self.window.beginSheet_completionHandler(&e.window, None);
        let first: &NSView = if current.is_some() { &e.url } else { &e.name };
        e.window.makeFirstResponder(Some(first));
    }

    pub fn ask_remove(self: &Rc<Self>) {
        let (Some(mtm), Some(n)) = (MainThreadMarker::new(), self.selected()) else { return };
        let Some(name) = super::stored_sources().get(n).map(Source::label) else { return };
        let me = self.clone();
        ui::sheet(
            Some(&self.window),
            ui::Style::Warning,
            &tr!(AskRemoveSource, name),
            "",
            &[tr!(BtnOk), tr!(BtnCancel)],
            move |b| {
                if b != 0 {
                    return;
                }
                let mut sources = super::stored_sources();
                if n < sources.len() {
                    sources.remove(n);
                    super::store(&sources);
                }
                me.say("", false);
                me.fill(Some(n.min(sources.len().saturating_sub(1))));
            },
            mtm,
        );
    }

    /// Test: asks the selected source for its categories on a thread.
    pub fn run_test(&self) {
        let Some(source) = self.selected().and_then(|n| super::stored_sources().get(n).cloned()) else {
            return;
        };
        let label = source.label();
        self.say(&tr!(SearchTesting, source = label), false);
        self.test.setEnabled(false);
        // a test of the helper's source starts it
        super::configure_helper();
        let proxy = peek(|c| c.proxy()).flatten();
        std::thread::spawn(move || {
            let key = source.api_key.clone();
            let (text, failed) = match fetch_caps(&source, proxy.as_ref()) {
                Ok(cats) => (tr!(SearchTestOk, source = label, count = cats.len()), false),
                Err(e) => (tr!(SearchFailed, source = label, error = redact(&e.to_string(), &key)), true),
            };
            crate::mainq::on_main(move || {
                if let Some(m) = open_window() {
                    m.test.setEnabled(m.selected().is_some());
                    m.say(&text, failed);
                }
                super::tour::tested();
            });
        });
    }
}

fn build_editor(current: Option<&Source>, index: Option<usize>, mtm: MainThreadMarker) -> Rc<Editor> {
    let id = dialogs::new_id();
    let name = ui::field(current.map_or("", |s| s.name.as_str()), "", mtm);
    let url = ui::field(current.map_or("", |s| s.url.as_str()), "", mtm);
    layout::min_width(&url, 360.0);
    // never the stored key: left empty, it stays
    let key = ui::secure_field("", mtm);
    if current.is_some_and(|s| !s.api_key.is_empty()) {
        key.setPlaceholderString(Some(&ns(&ui::text(tr!(SearchSourceKeyKept)))));
    }
    let enabled = ui::check(
        &ui::text(tr!(SearchSourceEnabled)),
        current.is_none_or(|s| s.enabled),
        cmd(id, NOTHING),
        mtm,
    );
    let hint = ui::note(tr!(SearchSourceUrlHint), mtm);
    hint.setSelectable(true);
    layout::width(&hint, 360.0);
    let caption = |s: &str| ui::label_tr(s, mtm);
    let (c_name, c_url, c_key) =
        (caption(tr!(SearchSourceName)), caption(tr!(SearchSourceUrl)), caption(tr!(SearchSourceKey)));
    let grid = layout::grid(
        &[
            vec![Some(&*c_name), Some(&*name)],
            vec![Some(&*c_url), Some(&*url)],
            vec![None, Some(&*hint)],
            vec![Some(&*c_key), Some(&*key)],
            vec![None, Some(&*enabled)],
        ],
        mtm,
    );
    layout::label_column(&grid);
    let ok = ui::button_tr(tr!(BtnOk), cmd(id, EDITOR_OK), mtm);
    let cancel = ui::button_tr(tr!(BtnCancel), cmd(id, EDITOR_CANCEL), mtm);
    let buttons = dialogs::buttons(&[], &cancel, &ok, mtm);
    let views: [&NSView; 2] = [&grid, &buttons];
    let root = dialogs::column(&views, mtm);
    root.setSpacing(14.0);
    let title = if current.is_some() { tr!(SearchEditSource) } else { tr!(SearchAddSource) };
    let window = dialogs::window(&ui::text(title), &root, None, false, mtm);
    let e = Rc::new(Editor { id, window, name, url, key, enabled, index });
    let handler = e.clone();
    dialogs::set_handler(id, move |act| handler.act(act));
    EDITOR.with(|o| *o.borrow_mut() = Some(e.clone()));
    e
}

impl Editor {
    fn act(&self, act: u8) {
        match act {
            EDITOR_OK => self.ok(),
            EDITOR_CANCEL => self.close(),
            _ => {}
        }
    }

    fn close(&self) {
        if let Some(p) = self.window.sheetParent() {
            p.endSheet(&self.window);
        }
        self.window.orderOut(None);
        EDITOR.with(|o| o.borrow_mut().take());
        dialogs::retire(self.id);
    }

    fn ok(&self) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let form = SourceForm {
            name: ui::value(&self.name),
            url: ui::value(&self.url),
            key: self.key.stringValue().to_string(),
            enabled: ui::checked(&self.enabled),
        };
        let mut sources = super::stored_sources();
        match apply_form(&mut sources, self.index, &form) {
            Ok(at) => {
                super::store(&sources);
                self.close();
                if let Some(m) = open_window() {
                    m.say("", false);
                    m.fill(Some(at));
                }
            }
            Err(text) => {
                self.window.makeFirstResponder(Some(&self.url));
                ui::sheet(Some(&self.window), ui::Style::Warning, &text, "", &[], |_| {}, mtm);
            }
        }
    }
}

/// A bold caption (the picker's and the jobs' headings).
pub fn bold(l: &NSTextField) {
    l.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
}
