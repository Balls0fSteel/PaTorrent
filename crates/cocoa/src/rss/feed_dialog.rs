//! Add RSS Feed and Edit Feed (GTK `rss::feed_dialog`): the feed's URL (with µTorrent's optional
//! `:COOKIE:` suffix), its alias, and the subscription (don't download automatically, or
//! download every item, with or without the smart episode filter). A sheet on the window it was
//! asked from; OK with an empty URL says so and stays. Add starts with the clipboard's text when
//! it is a feed URL.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSPasteboard, NSPasteboardTypeString, NSTextField, NSView, NSWindow};
use rookparcel_frontend::rss::view::{self, FeedForm};
use rookparcel_frontend::tr;

use crate::dialogs::{self, Observer, cmd};
use crate::ui::{self, layout, ns};

pub(super) const OK: u8 = 1;
pub(super) const CANCEL: u8 = 2;
/// A subscription radio button: "smart" follows "download all".
const SUBSCRIBE: u8 = 3;
const CLOSED: u8 = 4;

type Done = Box<dyn FnOnce(FeedForm)>;

pub(super) struct Dialog {
    id: u16,
    window: Retained<NSWindow>,
    /// The window it is a sheet on.
    parent: Option<Retained<NSWindow>>,
    pub(super) url: Retained<NSTextField>,
    pub(super) alias: Retained<NSTextField>,
    no_auto: Retained<NSButton>,
    auto: Retained<NSButton>,
    smart: Retained<NSButton>,
    done: RefCell<Option<Done>>,
    observer: RefCell<Option<Observer>>,
}

thread_local! {
    /// The dialog open now (for the RSS tour).
    static OPEN: RefCell<Weak<Dialog>> = const { RefCell::new(Weak::new()) };
}

/// The dialog open now.
pub(super) fn current() -> Option<Rc<Dialog>> {
    OPEN.with(|o| o.borrow().upgrade())
}

/// Add RSS Feed over `parent`, its URL from the clipboard when that holds one; `done` gets the
/// fields on OK.
pub(super) fn add(parent: Option<Retained<NSWindow>>, done: impl FnOnce(FeedForm) + 'static) {
    // SAFETY: the string type's constant.
    let clip = NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeString });
    let url = clip.and_then(|t| view::feed_url(&t.to_string())).unwrap_or_default();
    show(parent, tr!(RssAddFeed), FeedForm { url, ..FeedForm::default() }, done);
}

/// The dialog titled `title` with `form`'s fields, over `parent`; `done` gets them on OK.
pub(super) fn show(
    parent: Option<Retained<NSWindow>>,
    title: &str,
    form: FeedForm,
    done: impl FnOnce(FeedForm) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let id = dialogs::new_id();
    let url = ui::field(&form.url, "https://", mtm);
    layout::min_width(&url, 380.0);
    let alias = ui::field(&form.alias, "", mtm);
    let fields = layout::grid(
        &[
            vec![Some(&*ui::label_tr(tr!(RssFeedUrl), mtm)), Some(&*url)],
            vec![Some(&*ui::label_tr(tr!(RssAlias), mtm)), Some(&*alias)],
        ],
        mtm,
    );
    layout::label_column(&fields);
    let radio = |t: &str, on: bool| {
        // SAFETY: the target and action are set right after through `wire`; the two radio buttons
        // share their superview and action, so AppKit keeps one of them on.
        let b = unsafe { NSButton::radioButtonWithTitle_target_action(&ns(&ui::text(t)), None, None, mtm) };
        ui::set_checked(&b, on);
        ui::target::wire(&b, cmd(id, SUBSCRIBE), mtm);
        ui::auto(&b);
        b
    };
    let no_auto = radio(tr!(RssNoAuto), !form.subscribe);
    let auto = radio(tr!(RssAuto), form.subscribe);
    let smart = ui::check(&ui::text(tr!(RssSmartAuto)), form.smart, cmd(id, dialogs::NOTHING), mtm);
    smart.setEnabled(form.subscribe);
    let smart_row = layout::hstack(&[&smart], mtm);
    layout::insets(&smart_row, 0.0, 20.0, 0.0, 0.0);
    let choices = layout::vstack(&[&no_auto, &auto, &smart_row], mtm);
    let subscription = dialogs::section(tr!(RssSubscription), &choices, mtm);
    let ok = ui::button_tr(tr!(BtnOk), cmd(id, OK), mtm);
    let cancel = ui::button_tr(tr!(BtnCancel), cmd(id, CANCEL), mtm);
    let buttons = dialogs::buttons(&[], &cancel, &ok, mtm);
    let views: [&NSView; 3] = [&fields, &subscription, &buttons];
    let root = dialogs::column(&views, mtm);
    root.setSpacing(14.0);
    let window = dialogs::window(&ui::text(title), &root, None, false, mtm);
    dialogs::focus_first(&window, &url);
    let parent = parent.filter(|p| p.isVisible());
    let d = Rc::new(Dialog {
        id,
        window: window.clone(),
        parent: parent.clone(),
        url,
        alias,
        no_auto,
        auto,
        smart,
        done: RefCell::new(Some(Box::new(done))),
        observer: RefCell::new(None),
    });
    *d.observer.borrow_mut() = Some(dialogs::on_close(&window, id, CLOSED));
    let handler = d.clone();
    dialogs::set_handler(id, move |act| handler.act(act));
    OPEN.with(|o| *o.borrow_mut() = Rc::downgrade(&d));
    dialogs::test_accept(id, OK);
    match parent {
        Some(p) => p.beginSheet_completionHandler(&window, None),
        None => {
            window.center();
            window.makeKeyAndOrderFront(None);
        }
    }
}

impl Dialog {
    pub(super) fn act(&self, act: u8) {
        match act {
            OK => self.ok(),
            CANCEL | CLOSED => self.finish(None),
            SUBSCRIBE => self.smart.setEnabled(ui::checked(&self.auto)),
            _ => {}
        }
    }

    fn ok(&self) {
        let url = ui::value(&self.url).trim().to_owned();
        if url.is_empty() {
            if let Some(mtm) = MainThreadMarker::new() {
                dialogs::tell(Some(&self.window), ui::Style::Warning, &ui::text(tr!(RssNoUrl)), "", mtm);
            }
            self.window.makeFirstResponder(Some(&self.url));
            return;
        }
        let subscribe = ui::checked(&self.auto) && !ui::checked(&self.no_auto);
        let form = FeedForm {
            url,
            alias: ui::value(&self.alias).trim().to_owned(),
            subscribe,
            smart: subscribe && ui::checked(&self.smart),
        };
        self.finish(Some(form));
    }

    /// The dialog's end: it goes away, then `done` gets `form` (OK).
    fn finish(&self, form: Option<FeedForm>) {
        if let Some(o) = self.observer.borrow_mut().take() {
            dialogs::forget(&o);
        }
        dialogs::retire(self.id);
        OPEN.with(|o| *o.borrow_mut() = Weak::new());
        match &self.parent {
            Some(p) => p.endSheet(&self.window),
            None => self.window.close(),
        }
        let done = self.done.borrow_mut().take();
        if let (Some(f), Some(form)) = (done, form) {
            f(form);
        }
    }
}
