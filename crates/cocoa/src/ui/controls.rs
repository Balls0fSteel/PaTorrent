//! Controls with the settings every window wants: Auto Layout on (no autoresizing mask), texts
//! through [`super::text`] where they come from the string table, actions through `RPTarget`.
//! Every function takes the already translated text; pass `tr!(..)` through `text` first, or use
//! the `_tr` forms that do it.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSImage, NSImageView,
    NSLineBreakMode, NSPopUpButton, NSSearchField, NSSecureTextField, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::target::wire;
use super::{ns, text};
use crate::commands::Command;

/// `view` ready for Auto Layout.
pub fn auto(view: &NSView) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
}

/// A one-line label (not editable, not selectable).
pub fn label(s: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(s), mtm);
    auto(&l);
    l
}

/// [`label`] of a string-table text.
pub fn label_tr(s: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    label(&text(s), mtm)
}

/// A label that wraps at its width (set a width or pin its sides).
pub fn wrapping_label(s: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&ns(s), mtm);
    auto(&l);
    l
}

/// A label in the secondary (grey) label colour, at the small system size.
pub fn note(s: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = wrapping_label(s, mtm);
    l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    l.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
    l
}

/// A label that cuts its text with "…" at the end instead of growing (status bars, cells).
pub fn truncating_label(s: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = label(s, mtm);
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l
}

/// Right-aligned label (numbers).
pub fn align_right(l: &NSTextField) {
    l.setAlignment(NSTextAlignment::Right);
}

/// An editable text field; `placeholder` is the grey hint shown while it is empty.
pub fn field(value: &str, placeholder: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let f = NSTextField::textFieldWithString(&ns(value), mtm);
    if !placeholder.is_empty() {
        f.setPlaceholderString(Some(&ns(placeholder)));
    }
    auto(&f);
    f
}

/// A password field.
pub fn secure_field(value: &str, mtm: MainThreadMarker) -> Retained<NSSecureTextField> {
    let f = NSSecureTextField::new(mtm);
    f.setStringValue(&ns(value));
    auto(&f);
    f
}

/// A search field running `cmd` as its text changes (and on Return).
pub fn search_field(placeholder: &str, cmd: Command, mtm: MainThreadMarker) -> Retained<NSSearchField> {
    let f = NSSearchField::new(mtm);
    f.setPlaceholderString(Some(&ns(placeholder)));
    wire(&f, cmd, mtm);
    auto(&f);
    f
}

/// A field's text.
pub fn value(f: &NSTextField) -> String {
    f.stringValue().to_string()
}

/// A push button running `cmd`.
pub fn button(title: &str, cmd: Command, mtm: MainThreadMarker) -> Retained<NSButton> {
    // SAFETY: the target and action are set right after through `wire`; nil for now.
    let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
    wire(&b, cmd, mtm);
    auto(&b);
    b
}

/// [`button`] of a string-table text.
pub fn button_tr(title: &str, cmd: Command, mtm: MainThreadMarker) -> Retained<NSButton> {
    button(&text(title), cmd, mtm)
}

/// A check box running `cmd` when clicked.
pub fn check(title: &str, on: bool, cmd: Command, mtm: MainThreadMarker) -> Retained<NSButton> {
    // SAFETY: the target and action are set right after through `wire`; nil for now.
    let b = unsafe { NSButton::checkboxWithTitle_target_action(&ns(title), None, None, mtm) };
    set_checked(&b, on);
    wire(&b, cmd, mtm);
    auto(&b);
    b
}

/// A check box's state.
pub fn checked(b: &NSButton) -> bool {
    b.state() == NSControlStateValueOn
}

pub fn set_checked(b: &NSButton, on: bool) {
    b.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
}

/// A pop-up button of `items` with `selected` chosen, running `cmd` on a choice.
pub fn popup(
    items: &[String],
    selected: usize,
    cmd: Command,
    mtm: MainThreadMarker,
) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 24.0)),
        false,
    );
    for i in items {
        p.addItemWithTitle(&ns(i));
    }
    if selected < items.len() {
        p.selectItemAtIndex(selected as isize);
    }
    wire(&p, cmd, mtm);
    auto(&p);
    p
}

/// The pop-up's chosen index.
pub fn chosen(p: &NSPopUpButton) -> Option<usize> {
    usize::try_from(p.indexOfSelectedItem()).ok()
}

/// An image view of `image` at its own size.
pub fn image_view(image: &NSImage, mtm: MainThreadMarker) -> Retained<NSImageView> {
    let v = NSImageView::imageViewWithImage(image, mtm);
    auto(&v);
    v
}

/// The text of a control's tooltip.
pub fn tooltip(view: &NSView, tip: &str) {
    let tip = NSString::from_str(tip);
    view.setToolTip(if tip.length() == 0 { None } else { Some(&tip) });
}
