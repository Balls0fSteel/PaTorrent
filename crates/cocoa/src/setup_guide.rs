//! The Setup Guide (D53; GTK `setup_guide.rs`, `first_start_guide`, `setup_guide_later`): the
//! first start's few questions, one page at a time. The pages, what they set and their help come
//! from `frontend::setup_guide` (where downloads go; the router, with Keep ping low; the disks;
//! this Mac's autostart and associations); the controls are Preferences' fields, a question with
//! two or three answers as radio buttons, each field's help under it.
//!
//! - Options > Setup Guide ([`open`]): Finish applies what was changed the way Preferences does
//!   (the desktop integration, then `Core::apply_options`); Skip or closing changes nothing.
//! - A profile's first start ([`first_start`], from `app::start` before the engine starts, when
//!   `prepare_options` says so): Finish stores the answers in the new `rookparcel.ini`, Skip
//!   nothing; either way the guide is marked as seen, the file saved and the start goes on.
//!   Quitting instead leaves the file empty, so the guide comes again at the next start.
//!
//! Test aid (unset in normal use): with `ROOKPARCEL_TEST_ACCEPT=N` the guide goes to the next
//! page every N seconds and presses Finish on the last, as the torrent dialogs press their
//! default button (`dialogs`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBox, NSBoxType, NSButton, NSControl, NSFont, NSPopUpButton, NSTabView, NSTabViewItem, NSTabViewType,
    NSTextAlignment, NSTextField, NSView, NSWindow,
};
use objc2_foundation::NSSize;
use rookparcel_frontend::lang::{self, H};
use rookparcel_frontend::options::Options;
use rookparcel_frontend::pref_pages::{self, Control, Features, Field, Item, Page, TextKind};
use rookparcel_frontend::prefs::Prefs;
use rookparcel_frontend::setup_guide::{self, Answers};
use rookparcel_frontend::tr;

use crate::app::{apply, peek, with_core};
use crate::dialogs::{self, Observer, cmd};
use crate::ui::{self, layout, ns};

const SKIP: u8 = 1;
const BACK: u8 = 2;
const NEXT: u8 = 3;
const FINISH: u8 = 4;
/// The window closed.
const CLOSED: u8 = 5;
/// A check box changed: what depends on it is greyed or not.
const GREY: u8 = 6;
/// `ROOKPARCEL_TEST_ACCEPT`: the next page, or Finish on the last.
const TEST: u8 = 7;
/// A Browse button: this plus its index in `Guide::browse`.
const BROWSE: u8 = 16;

/// The widest a page's text wraps at.
const TEXT_WIDTH: f64 = 480.0;
/// How far a radio button, an indented field and a help text move in.
const INDENT: f64 = 20.0;

/// A control of the guide.
enum Ctl {
    Check(Retained<NSButton>),
    Text(Retained<NSTextField>),
    Popup(Retained<NSPopUpButton>, Vec<String>),
    /// A question's radio buttons and their answers' values.
    Options(Vec<Retained<NSButton>>, Vec<String>),
}

impl Ctl {
    fn value(&self) -> String {
        match self {
            Ctl::Check(b) => ui::checked(b).to_string(),
            Ctl::Text(t) => ui::value(t),
            Ctl::Popup(p, values) => ui::chosen(p).and_then(|i| values.get(i)).cloned().unwrap_or_default(),
            Ctl::Options(buttons, values) => {
                let i = buttons.iter().position(|b| ui::checked(b)).unwrap_or(0);
                values.get(i).cloned().unwrap_or_default()
            }
        }
    }

    fn set(&self, value: &str) {
        match self {
            Ctl::Check(b) => ui::set_checked(b, value == "true"),
            Ctl::Text(t) => t.setStringValue(&ns(value)),
            Ctl::Popup(p, values) => {
                if let Some(i) = values.iter().position(|v| v == value) {
                    p.selectItemAtIndex(i as isize);
                }
            }
            Ctl::Options(buttons, values) => {
                let i = values.iter().position(|v| v == value).unwrap_or(0);
                for (j, b) in buttons.iter().enumerate() {
                    ui::set_checked(b, j == i);
                }
            }
        }
    }

    /// The control that takes the focus.
    fn view(&self) -> Option<Retained<NSView>> {
        Some(match self {
            Ctl::Check(b) => Retained::into_super(Retained::into_super(b.clone())),
            Ctl::Text(t) => Retained::into_super(Retained::into_super(t.clone())),
            Ctl::Popup(p, _) => Retained::into_super(Retained::into_super(Retained::into_super(p.clone()))),
            Ctl::Options(b, _) => Retained::into_super(Retained::into_super(b.first()?.clone())),
        })
    }
}

/// What the guide gives back when it closes: the new options on Finish, `None` on Skip.
type Done = Box<dyn FnOnce(Option<Options>)>;

/// What building the pages collects.
struct Built {
    controls: HashMap<&'static str, Ctl>,
    /// The controls greyed unless every key's box is ticked.
    needs: Vec<(Vec<Retained<NSControl>>, &'static [&'static str])>,
    /// The Browse buttons' fields: (key, the picker's title, folders).
    browse: Vec<(&'static str, String, bool)>,
}

struct Guide {
    id: u16,
    window: Retained<NSWindow>,
    guide: setup_guide::Guide,
    built: Built,
    tabs: Retained<NSTabView>,
    page: Cell<usize>,
    back: Retained<NSButton>,
    next: Retained<NSButton>,
    options: Options,
    done: RefCell<Option<Done>>,
    observer: RefCell<Option<Observer>>,
}

thread_local! {
    /// The guide on screen.
    static OPEN: RefCell<Option<Rc<Guide>>> = const { RefCell::new(None) };
}

/// What the guide offers on the Mac: the last page (autostart, and the associations where they
/// can be set: from the bundle, in the default profile); none of it in a remote window.
fn features(remote: bool) -> Features {
    Features { desktop: !remote, packaged: !crate::mac::associations_settable(), ..Features::default() }
}

/// Options > Setup Guide: answers applied as Preferences applies them.
pub fn open() {
    if let Some(g) = OPEN.with(|o| o.borrow().clone()) {
        g.window.makeKeyAndOrderFront(None);
        return;
    }
    let Some((mut options, remote)) = peek(|c| (c.options(), c.is_remote())) else { return };
    options.integration = crate::mac::integration();
    show(options, remote, |answer| {
        let Some(o) = answer else { return };
        let old = crate::mac::integration();
        if o.integration != old {
            // (the message is whole: `set_integration` puts it in `ErrDesktopIntegration`)
            if let Err(e) = crate::mac::set_integration(&old, &o.integration) {
                if let Some(mtm) = MainThreadMarker::new() {
                    dialogs::tell(None, ui::Style::Error, &e, "", mtm);
                }
            }
        }
        apply(with_core(|c| c.apply_options(&o)));
    });
}

/// A profile's first start (D53): the guide as the only window, before the engine starts.
/// Finish stores its answers in `prefs` (the desktop integration in the system), Skip nothing;
/// either way the guide is marked as seen, `prefs` is saved and `then` goes on with the start.
pub fn first_start(mut options: Options, prefs: Prefs, then: impl FnOnce() + 'static) {
    options.integration = crate::mac::integration();
    show(options, false, move |answer| {
        let mut prefs = prefs;
        if let Some(o) = &answer {
            o.store(&mut prefs);
            let set = crate::mac::integration();
            if o.integration.normalized() != set.normalized() {
                if let Err(e) = crate::mac::set_integration(&set, &o.integration) {
                    eprintln!("{e}");
                }
            }
        }
        setup_guide::mark_seen(&mut prefs);
        if let Err(e) = prefs.save() {
            eprintln!("cannot save the settings: {e}");
        }
        then();
    });
}

/// Shows the guide for `options` (its `integration` as the system has it). `done` gets the new
/// options on Finish, `None` on Skip or when the window is closed.
fn show(options: Options, remote: bool, done: impl FnOnce(Option<Options>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let id = dialogs::new_id();
    let mut guide = setup_guide::Guide::new(features(remote));
    let count = guide.pages.len();
    let mut built = Built { controls: HashMap::new(), needs: Vec::new(), browse: Vec::new() };
    let tabs = NSTabView::new(mtm);
    tabs.setTabViewType(NSTabViewType::NoTabsNoBorder);
    ui::auto(&tabs);
    let (mut width, mut height) = (TEXT_WIDTH, 0.0_f64);
    for (i, page) in guide.pages.iter().enumerate() {
        let stack = page_view(id, &mut built, page, i, count, &options, mtm);
        let size = stack.fittingSize();
        width = width.max(size.width);
        height = height.max(size.height);
        // the tab view sizes the item's view by its frame; the page sits at its top
        let holder = NSView::new(mtm);
        holder.addSubview(&stack);
        layout::activate(&[
            stack.topAnchor().constraintEqualToAnchor(&holder.topAnchor()),
            stack.leadingAnchor().constraintEqualToAnchor(&holder.leadingAnchor()),
            stack.trailingAnchor().constraintEqualToAnchor(&holder.trailingAnchor()),
            holder.bottomAnchor().constraintGreaterThanOrEqualToAnchor(&stack.bottomAnchor()),
        ]);
        let item = NSTabViewItem::new();
        item.setView(Some(&holder));
        tabs.addTabViewItem(&item);
    }
    layout::min_width(&tabs, width);
    layout::activate(&[tabs.heightAnchor().constraintGreaterThanOrEqualToConstant(height)]);
    let values = guide.write(&options);

    let skip = ui::button_tr(tr!(GuideSkip), cmd(id, SKIP), mtm);
    skip.setKeyEquivalent(&ns("\u{1b}"));
    let back = ui::button_tr(tr!(GuideBack), cmd(id, BACK), mtm);
    let next = ui::button_tr(tr!(GuideNext), cmd(id, NEXT), mtm);
    let finish = ui::button_tr(tr!(GuideFinish), cmd(id, FINISH), mtm);
    finish.setKeyEquivalent(&ns("\r"));
    let spacer = layout::spacer(mtm);
    let buttons = layout::hstack(&[&skip, &spacer, &back, &next, &finish], mtm);
    let separator = NSBox::new(mtm);
    separator.setBoxType(NSBoxType::Separator);
    ui::auto(&separator);
    let views: [&NSView; 3] = [&tabs, &separator, &buttons];
    let root = dialogs::column(&views, mtm);
    root.setSpacing(12.0);
    let window =
        dialogs::window(&ui::text(tr!(GuideTitle)), &root, Some(NSSize::new(560.0, 420.0)), false, mtm);

    let g = Rc::new(Guide {
        id,
        window: window.clone(),
        guide,
        built,
        tabs,
        page: Cell::new(0),
        back,
        next,
        options,
        done: RefCell::new(Some(Box::new(done))),
        observer: RefCell::new(None),
    });
    for (key, value) in values {
        if let Some(c) = g.built.controls.get(key) {
            c.set(&value);
        }
    }
    *g.observer.borrow_mut() = Some(dialogs::on_close(&window, id, CLOSED));
    let handler = g.clone();
    dialogs::set_handler(id, move |act| handler.act(act));
    OPEN.with(|o| *o.borrow_mut() = Some(g.clone()));
    g.update_greying();
    g.show_page(0);
    dialogs::test_accept(id, TEST);
    window.center();
    window.makeKeyAndOrderFront(None);
    crate::mac::lifecycle::activate(mtm);
}

/// A wrapping label at the page's width.
fn text(s: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = ui::wrapping_label(s, mtm);
    l.setPreferredMaxLayoutWidth(TEXT_WIDTH);
    l
}

/// A field's help, grey and small, moved in by `indent`.
fn help_view(h: H, indent: f64, mtm: MainThreadMarker) -> Retained<NSView> {
    let l = ui::note(lang::help(h), mtm);
    l.setPreferredMaxLayoutWidth(TEXT_WIDTH - indent);
    moved_in(&l, indent, mtm)
}

/// `view` in a stack that moves it in by `indent`.
fn moved_in(view: &NSView, indent: f64, mtm: MainThreadMarker) -> Retained<NSView> {
    let s = layout::hstack(&[view], mtm);
    layout::insets(&s, 0.0, indent, 0.0, 0.0);
    Retained::into_super(s)
}

/// One page: the step, the title, the page's text, then each field with its help under it.
fn page_view(
    id: u16,
    b: &mut Built,
    page: &Page,
    index: usize,
    count: usize,
    o: &Options,
    mtm: MainThreadMarker,
) -> Retained<NSView> {
    let mut views: Vec<Retained<NSView>> = Vec::new();
    let step = ui::note(&setup_guide::step_text(index, count), mtm);
    views.push(Retained::into_super(Retained::into_super(step)));
    let title = ui::label(&ui::text(lang::text(page.title)), mtm);
    title.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize() + 3.0)));
    views.push(Retained::into_super(Retained::into_super(title)));
    let intro = text(lang::text(setup_guide::page_text(page)), mtm);
    views.push(Retained::into_super(Retained::into_super(intro)));
    let mut last_help = None;
    for item in &page.items {
        match item {
            Item::Field(f) => {
                let h = f.help_key();
                let question = setup_guide::as_options(&f.control, o);
                let is_question = question.is_some();
                field(id, b, f, question, o, &mut views, mtm);
                // a field without a caption shares the help above it: once is enough; a
                // question's answers have the page's note under them (and the help as tooltip)
                let h = if is_question { None } else { h };
                if let Some(h) = h.filter(|h| last_help != Some(*h)) {
                    views.push(help_view(h, if f.indent { 2.0 * INDENT } else { INDENT }, mtm));
                }
                last_help = h;
            }
            Item::Note(note) => {
                let l = ui::note(&note(), mtm);
                l.setPreferredMaxLayoutWidth(TEXT_WIDTH);
                views.push(Retained::into_super(Retained::into_super(l)));
            }
            _ => {}
        }
    }
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let stack = layout::vstack(&refs, mtm);
    stack.setSpacing(layout::SPACING);
    Retained::into_super(stack)
}

/// One field's controls (and caption, Browse button), added to `views`.
fn field(
    id: u16,
    b: &mut Built,
    f: &Field,
    question: Option<Vec<(String, String)>>,
    o: &Options,
    views: &mut Vec<Retained<NSView>>,
    mtm: MainThreadMarker,
) {
    let h = f.help_key();
    let label = f.label.map(|s| ui::text(lang::text(s)));
    let tip = |v: &NSView| {
        if let Some(h) = h {
            ui::tooltip(v, lang::help(h));
        }
    };
    let mut grey: Vec<Retained<NSControl>> = Vec::new();
    let ctl = if let Some(choices) = question {
        if let Some(l) = &label {
            views.push(Retained::into_super(Retained::into_super(text(l, mtm))));
        }
        let mut buttons = Vec::new();
        let mut values = Vec::new();
        for (value, title) in choices {
            // SAFETY: the target and action are set right after through `wire`; nil for now.
            let r = unsafe { NSButton::radioButtonWithTitle_target_action(&ns(&title), None, None, mtm) };
            ui::target::wire(&r, cmd(id, dialogs::NOTHING), mtm);
            ui::auto(&r);
            tip(&r);
            grey.push(Retained::into_super(r.clone()));
            buttons.push(r);
            values.push(value);
        }
        // their own stack: AppKit groups the radio buttons of one superview
        let refs: Vec<&NSView> = buttons.iter().map(|r| &**r as &NSView).collect();
        let group = layout::vstack(&refs, mtm);
        layout::insets(&group, 0.0, INDENT, 0.0, 0.0);
        views.push(Retained::into_super(group));
        Ctl::Options(buttons, values)
    } else {
        match f.control {
            Control::Check { .. } => {
                let c = ui::check(label.as_deref().unwrap_or(""), false, cmd(id, GREY), mtm);
                tip(&c);
                grey.push(Retained::into_super(c.clone()));
                views.push(moved_in(&c, if f.indent { INDENT } else { 0.0 }, mtm));
                Ctl::Check(c)
            }
            Control::Choice { choices, .. } => {
                let list = choices(o);
                let texts: Vec<String> = list.iter().map(|(_, t)| t.clone()).collect();
                let p = ui::popup(&texts, 0, cmd(id, dialogs::NOTHING), mtm);
                tip(&p);
                grey.push(Retained::into_super(Retained::into_super(p.clone())));
                caption_row(label.as_deref(), &p, f.indent, views, mtm);
                Ctl::Popup(p, list.into_iter().map(|(v, _)| v).collect())
            }
            Control::Number { .. } => {
                let t = ui::field("", "", mtm);
                t.setAlignment(NSTextAlignment::Right);
                layout::width(&t, 90.0);
                tip(&t);
                grey.push(Retained::into_super(t.clone()));
                caption_row(label.as_deref(), &t, f.indent, views, mtm);
                Ctl::Text(t)
            }
            Control::Text { kind, .. } => {
                let t = ui::field("", "", mtm);
                layout::min_width(&t, 320.0);
                layout::stretch(&t);
                tip(&t);
                grey.push(Retained::into_super(t.clone()));
                let mut parts: Vec<Retained<NSView>> =
                    vec![Retained::into_super(Retained::into_super(t.clone()))];
                if let TextKind::Folder(title) | TextKind::File(title) = kind {
                    let folders = matches!(kind, TextKind::Folder(_));
                    let index = b.browse.len() as u8;
                    b.browse.push((f.key, ui::text(lang::text(title)), folders));
                    let browse = ui::button("", cmd(id, BROWSE + index), mtm);
                    let symbol = if folders { "folder" } else { "doc" };
                    match crate::images::symbol(symbol, &ui::text(lang::text(title))) {
                        Some(image) => {
                            browse.setImage(Some(&image));
                            browse.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageOnly);
                        }
                        None => browse.setTitle(&ns("\u{2026}")),
                    }
                    tip(&browse);
                    grey.push(Retained::into_super(browse.clone()));
                    parts.push(Retained::into_super(Retained::into_super(browse)));
                }
                let refs: Vec<&NSView> = parts.iter().map(|p| &**p).collect();
                let row = layout::hstack(&refs, mtm);
                // a folder under its check box moves in
                let indent = if f.indent || f.label.is_none() { INDENT } else { 0.0 };
                if let Some(l) = &label {
                    let c = ui::label(l, mtm);
                    grey.push(Retained::into_super(c.clone()));
                    views.push(moved_in(&c, indent, mtm));
                }
                views.push(moved_in(&row, indent, mtm));
                Ctl::Text(t)
            }
        }
    };
    if f.fixed {
        for c in &grey {
            c.setEnabled(false);
        }
    } else if !f.needs.is_empty() {
        b.needs.push((grey, f.needs));
    }
    b.controls.insert(f.key, ctl);
}

/// A caption and its control in one row.
fn caption_row(
    label: Option<&str>,
    control: &NSView,
    indent: bool,
    views: &mut Vec<Retained<NSView>>,
    mtm: MainThreadMarker,
) {
    let row = match label {
        Some(l) => {
            let c = ui::label(l, mtm);
            layout::hstack(&[&c, control], mtm)
        }
        None => layout::hstack(&[control], mtm),
    };
    views.push(moved_in(&row, if indent { INDENT } else { 0.0 }, mtm));
}

impl Guide {
    fn act(self: &Rc<Self>, act: u8) {
        match act {
            SKIP => self.window.close(),
            BACK => self.show_page(self.page.get().saturating_sub(1)),
            NEXT => self.show_page(self.page.get() + 1),
            FINISH => self.finish(),
            CLOSED => self.closed(),
            GREY => self.update_greying(),
            TEST => {
                let page = self.page.get();
                if page + 1 < self.guide.pages.len() {
                    eprintln!("ROOKPARCEL_TEST_ACCEPT: Setup Guide page {}", page + 2);
                    self.show_page(page + 1);
                    dialogs::test_accept(self.id, TEST);
                } else {
                    self.finish();
                }
            }
            a if a >= BROWSE => self.browse(usize::from(a - BROWSE)),
            _ => {}
        }
    }

    fn show_page(&self, page: usize) {
        let count = self.guide.pages.len();
        let page = page.min(count.saturating_sub(1));
        self.page.set(page);
        self.tabs.selectTabViewItemAtIndex(page as isize);
        let (back, next) = setup_guide::nav(page, count);
        self.back.setEnabled(back);
        self.next.setEnabled(next);
    }

    /// Greys what needs a box that is not ticked.
    fn update_greying(&self) {
        let on = |k: &str| {
            let Some(ctl) = self.built.controls.get(k) else { return false };
            let value = ctl.value();
            pref_pages::fields(&self.guide.pages).any(|f| f.key == k && pref_pages::is_on(&f.control, &value))
        };
        for (controls, needs) in &self.built.needs {
            let enabled = needs.iter().all(|k| on(k));
            for c in controls {
                c.setEnabled(enabled);
            }
        }
    }

    fn answers(&self) -> Answers {
        self.built.controls.iter().map(|(k, c)| (*k, c.value())).collect()
    }

    /// Finish: the answers applied to the options, or the page of a value that cannot be
    /// stored, with its message.
    fn finish(&self) {
        let mut o = self.options.clone();
        match self.guide.apply(&self.answers(), &mut o) {
            Ok(()) => {
                // taken first: `done` may open windows, and the close asks again
                let done = self.done.borrow_mut().take();
                self.window.close();
                if let Some(done) = done {
                    done(Some(o));
                }
            }
            Err((key, message)) => {
                if let Some(page) = self.guide.page_of(key) {
                    self.show_page(page);
                }
                if let Some(v) = self.built.controls.get(key).and_then(Ctl::view) {
                    self.window.makeFirstResponder(Some(&v));
                }
                let Some(mtm) = MainThreadMarker::new() else { return };
                ui::sheet(Some(&self.window), ui::Style::Warning, &message, "", &[], |_| {}, mtm);
            }
        }
    }

    /// A folder or file field's panel.
    fn browse(&self, index: usize) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some((key, title, folders)) = self.built.browse.get(index).cloned() else { return };
        let Some(Ctl::Text(field)) = self.built.controls.get(key) else { return };
        let now = PathBuf::from(ui::value(field).trim());
        let start = if now.is_dir() { Some(now) } else { now.parent().map(|p| p.to_path_buf()) };
        let pick = ui::Pick {
            files: !folders,
            folders,
            start: start.filter(|p| p.is_dir()),
            message: title,
            ..ui::Pick::default()
        };
        let field = field.clone();
        dialogs::open_panel(
            Some(&self.window),
            pick,
            move |paths| {
                if let Some(p) = paths.first() {
                    field.setStringValue(&ns(&p.to_string_lossy()));
                }
            },
            mtm,
        );
    }

    /// The window closed (Skip, Escape, the close button, or after Finish).
    fn closed(&self) {
        if let Some(o) = self.observer.borrow_mut().take() {
            dialogs::forget(&o);
        }
        dialogs::retire(self.id);
        OPEN.with(|o| o.borrow_mut().take());
        let done = self.done.borrow_mut().take();
        // Quit is not Skip: a first start's settings stay unsaved, so the guide asks again (as
        // GTK, whose quit never closes the window through its close request)
        if crate::mac::lifecycle::quitting() {
            return;
        }
        if let Some(done) = done {
            done(None);
        }
    }
}
