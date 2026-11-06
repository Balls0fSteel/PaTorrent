//! A page's controls, built from its `pref_pages` items (no page is drawn by hand): an
//! `NSGridView` of two columns, the captions right-aligned before their controls and the check
//! boxes in the second column, as Mac forms have them. A group is a bold heading across both
//! columns; a field `beside` the one before it joins that row; an indented one moves right; a
//! text field has a hidden row under it for its message. The grid sits in a scroll view whose
//! width follows the window.

use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, Message};
use objc2_app_kit::{
    NSCellImagePosition, NSControl, NSFont, NSGridCellPlacement, NSGridRowAlignment, NSLayoutConstraint,
    NSScrollView, NSStackView, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRange, NSRect, NSSize};
use rookparcel_frontend::lang::{self, H};
use rookparcel_frontend::options::Options;
use rookparcel_frontend::pref_pages::{self, Control, Field, Item, Page, TextKind};

use super::{Act, Ctl, FieldUi, advanced, sched_grid};
use crate::canvas::RPCanvas;
use crate::commands::Command;
use crate::ui::{self, layout};

/// How far an indented control moves (about a check box's width and gap).
const INDENT: f64 = 20.0;
/// The widest a note wraps at.
const NOTE_WIDTH: f64 = 440.0;

/// One row of a page's grid.
enum Row {
    /// A caption (or none) and the controls.
    Pair(Option<Retained<NSView>>, Retained<NSView>),
    /// A group's heading, across both columns.
    Group(Retained<NSView>),
    /// A table or the scheduler grid, across both columns.
    Wide(Retained<NSView>),
    /// The message row under a text field (hidden until there is one).
    Error(&'static str, Retained<NSTextField>),
}

/// What building the pages collects for the window.
pub(crate) struct Builder {
    pub mtm: MainThreadMarker,
    pub acts: Vec<Act>,
    pub fields: HashMap<&'static str, FieldUi>,
    pub helps: Vec<(Retained<NSView>, H)>,
    pub ip_status: Option<Retained<NSTextField>>,
    pub advanced: Option<advanced::Advanced>,
    pub sched: Option<Retained<RPCanvas>>,
    /// The widest page's grid (the window is made wide enough for it).
    pub widest: f64,
}

impl Builder {
    pub fn new(mtm: MainThreadMarker) -> Builder {
        Builder {
            mtm,
            acts: Vec::new(),
            fields: HashMap::new(),
            helps: Vec::new(),
            ip_status: None,
            advanced: None,
            sched: None,
            widest: 0.0,
        }
    }

    /// The command a control runs for `act`.
    pub fn act(&mut self, act: Act) -> Command {
        self.acts.push(act);
        Command::Pref((self.acts.len() - 1) as u16)
    }

    /// Gives `view` the help `h`: its tooltip, and the help line's text while it is hovered or
    /// focused.
    pub fn help(&mut self, view: &NSView, h: Option<H>) {
        let Some(h) = h else { return };
        ui::tooltip(view, lang::help(h));
        self.helps.push((view.retain(), h));
    }

    /// The page's view: its grid in a scroll view.
    pub fn page(&mut self, page: &Page, o: &Options) -> Retained<NSView> {
        let mtm = self.mtm;
        let mut rows = Vec::new();
        // the row a `beside` field or a button joins
        let mut last: Option<Retained<NSStackView>> = None;
        for item in &page.items {
            match item {
                Item::Group(title) => {
                    let l = ui::label(&ui::text(lang::text(*title)), mtm);
                    l.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
                    rows.push(Row::Group(into_view(&l)));
                    last = None;
                }
                Item::Field(f) => self.field(f, o, &mut rows, &mut last),
                Item::Note(text) => {
                    let l = ui::note(&note_text(text()), mtm);
                    l.setPreferredMaxLayoutWidth(NOTE_WIDTH);
                    rows.push(Row::Pair(None, into_view(&l)));
                    last = None;
                }
                Item::Button { action, label } => {
                    let cmd = self.act(Act::Button(*action));
                    let b = ui::button_tr(lang::text(*label), cmd, mtm);
                    self.help(&b, pref_pages::item_help(item));
                    match &last {
                        Some(row) => row.addArrangedSubview(&b),
                        None => {
                            let row = row_stack(&[&b], mtm);
                            rows.push(Row::Pair(None, into_view(&row)));
                            last = Some(row);
                        }
                    }
                }
                Item::IpFilterStatus => {
                    let l = ui::wrapping_label("", mtm);
                    l.setPreferredMaxLayoutWidth(NOTE_WIDTH);
                    rows.push(Row::Pair(None, into_view(&l)));
                    self.ip_status = Some(l);
                    last = None;
                }
                Item::AdvancedTable => {
                    let view = advanced::build(self);
                    rows.push(Row::Wide(view));
                    last = None;
                }
                Item::SchedTable => {
                    let canvas = sched_grid::build(mtm);
                    self.help(&canvas, pref_pages::item_help(item));
                    self.sched = Some(canvas.clone());
                    rows.push(Row::Wide(Retained::into_super(canvas)));
                    last = None;
                }
            }
        }
        self.grid_page(rows)
    }

    /// One field: its control (and caption, Browse button, message row).
    fn field(
        &mut self,
        f: &Field,
        o: &Options,
        rows: &mut Vec<Row>,
        last: &mut Option<Retained<NSStackView>>,
    ) {
        let mtm = self.mtm;
        let h = f.help_key();
        let cmd = self.act(Act::Field(f.key));
        let label_text = f.label.map(|s| ui::text(lang::text(s))).unwrap_or_default();
        let mut grey: Vec<Retained<NSControl>> = Vec::new();
        let ctl = match f.control {
            Control::Check { .. } => Ctl::Check(ui::check(&label_text, false, cmd, mtm)),
            Control::Number { .. } => {
                let t = ui::field("", "", mtm);
                t.setAlignment(NSTextAlignment::Right);
                layout::width(&t, 90.0);
                Ctl::Text(t)
            }
            Control::Text { kind, .. } => {
                let t = if kind == TextKind::Password {
                    Retained::into_super(ui::secure_field("", mtm))
                } else {
                    ui::field("", "", mtm)
                };
                layout::min_width(&t, 260.0);
                layout::stretch(&t);
                Ctl::Text(t)
            }
            Control::Choice { choices, .. } => {
                let list = choices(o);
                let texts: Vec<String> = list.iter().map(|(_, t)| t.clone()).collect();
                let p = ui::popup(&texts, 0, cmd, mtm);
                Ctl::Choice(p, list.into_iter().map(|(v, _)| v).collect())
            }
        };
        if let Ctl::Text(t) = &ctl {
            crate::ui::target::wire(t, cmd, mtm);
            // Return, Tab, a click elsewhere and the window closing all commit
            if let Some(cell) = t.cell() {
                cell.setSendsActionOnEndEditing(true);
            }
        }
        let view = ctl.view().retain();
        self.help(&view, h);
        grey.push(control_of(&ctl));
        let mut parts: Vec<Retained<NSView>> = vec![view];
        if let Control::Text { kind: TextKind::Folder(title) | TextKind::File(title), .. } = f.control {
            let browse_cmd = self.act(Act::Browse(f.key));
            let b = ui::button("", browse_cmd, mtm);
            let folder = matches!(f.control, Control::Text { kind: TextKind::Folder(_), .. });
            let symbol = if folder { "folder" } else { "doc" };
            if let Some(image) = crate::images::symbol(symbol, &ui::text(lang::text(title))) {
                b.setImage(Some(&image));
                b.setImagePosition(NSCellImagePosition::ImageOnly);
            } else {
                b.setTitle(&ui::ns("\u{2026}"));
            }
            self.help(&b, h);
            grey.push(Retained::into_super(b.clone()));
            parts.push(into_view(&b));
        }
        let is_text = matches!(f.control, Control::Number { .. } | Control::Text { .. });
        match last.as_ref().filter(|_| f.beside) {
            Some(row) => {
                for p in &parts {
                    row.addArrangedSubview(p);
                }
            }
            None => {
                let refs: Vec<&NSView> = parts.iter().map(|p| &**p).collect();
                let content = row_stack(&refs, mtm);
                let is_check = matches!(f.control, Control::Check { .. });
                // under its check box: a folder without a caption, an indented box
                if (f.indent && is_check) || (f.label.is_none() && !is_check) {
                    layout::insets(&content, 0.0, INDENT, 0.0, 0.0);
                }
                let caption = match (f.label, is_check) {
                    (Some(_), false) => {
                        let l = ui::label(&label_text, mtm);
                        self.help(&l, h);
                        grey.push(Retained::into_super(l.clone()));
                        Some(into_view(&l))
                    }
                    _ => None,
                };
                rows.push(Row::Pair(caption, into_view(&content)));
                *last = Some(content);
            }
        }
        if is_text {
            let l = ui::note("", mtm);
            l.setTextColor(Some(&super::error_color()));
            l.setPreferredMaxLayoutWidth(NOTE_WIDTH);
            rows.push(Row::Error(f.key, l));
        }
        // the message's row is set once the grid is made (`grid_page`)
        self.fields.insert(f.key, FieldUi { field: *f, ctl, grey, error: None });
    }

    /// The rows in a grid, in a scroll view that follows the window's width.
    fn grid_page(&mut self, rows: Vec<Row>) -> Retained<NSView> {
        let mtm = self.mtm;
        let cells: Vec<Vec<Option<&NSView>>> = rows
            .iter()
            .map(|r| match r {
                Row::Pair(caption, content) => vec![caption.as_deref(), Some(&**content)],
                Row::Group(v) | Row::Wide(v) => vec![Some(&**v), None],
                Row::Error(_, l) => vec![None, Some(as_view(l))],
            })
            .collect();
        let grid = layout::grid(&cells, mtm);
        layout::label_column(&grid);
        grid.setRowAlignment(NSGridRowAlignment::FirstBaseline);
        for (i, r) in rows.iter().enumerate() {
            let row = grid.rowAtIndex(i as isize);
            match r {
                Row::Group(_) | Row::Wide(_) => {
                    row.mergeCellsInRange(NSRange::new(0, 2));
                    let cell = grid.cellAtColumnIndex_rowIndex(0, i as isize);
                    cell.setXPlacement(if matches!(r, Row::Wide(_)) {
                        NSGridCellPlacement::Fill
                    } else {
                        NSGridCellPlacement::Leading
                    });
                    row.setRowAlignment(NSGridRowAlignment::None);
                    if matches!(r, Row::Group(_)) && i > 0 {
                        row.setTopPadding(12.0);
                    }
                }
                Row::Error(key, label) => {
                    row.setHidden(true);
                    if let Some(f) = self.fields.get_mut(key) {
                        f.error = Some((label.clone(), row));
                    }
                }
                Row::Pair(..) => {}
            }
        }
        grid.columnAtIndex(1).setXPlacement(NSGridCellPlacement::Leading);
        // every caption and control inside the grid, so its least width holds them all (a
        // page of check boxes alone has an empty first column)
        let mut inside = Vec::new();
        for r in &rows {
            let views: Vec<&NSView> = match r {
                Row::Pair(caption, content) => caption.iter().map(|c| &**c).chain([&**content]).collect(),
                Row::Error(..) | Row::Group(_) | Row::Wide(_) => Vec::new(),
            };
            for v in views {
                inside.push(v.leadingAnchor().constraintGreaterThanOrEqualToAnchor(&grid.leadingAnchor()));
                inside.push(v.trailingAnchor().constraintLessThanOrEqualToAnchor(&grid.trailingAnchor()));
            }
        }
        layout::activate(&inside);
        self.widest = self.widest.max(grid.fittingSize().width);
        // the page: the grid at its natural size at the top left of a view as wide as the
        // scroll view and at least as tall
        let doc = NSView::new(mtm);
        ui::controls::auto(&doc);
        doc.addSubview(&grid);
        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setDocumentView(Some(&doc));
        let clip = scroll.contentView();
        let m = layout::MARGIN;
        let weak = |c: Retained<NSLayoutConstraint>, p: f32| {
            c.setPriority(p);
            c
        };
        layout::activate(&[
            grid.topAnchor().constraintEqualToAnchor_constant(&doc.topAnchor(), m),
            grid.leadingAnchor().constraintEqualToAnchor_constant(&doc.leadingAnchor(), m),
            doc.trailingAnchor().constraintGreaterThanOrEqualToAnchor_constant(&grid.trailingAnchor(), m),
            doc.bottomAnchor().constraintGreaterThanOrEqualToAnchor_constant(&grid.bottomAnchor(), m),
            // the grid as small as its rows let it be
            weak(grid.widthAnchor().constraintEqualToConstant(0.0), 1.0),
            weak(grid.heightAnchor().constraintEqualToConstant(0.0), 1.0),
            doc.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()),
            doc.trailingAnchor().constraintEqualToAnchor(&clip.trailingAnchor()),
            doc.heightAnchor().constraintGreaterThanOrEqualToAnchor(&clip.heightAnchor()),
            weak(doc.heightAnchor().constraintEqualToAnchor(&clip.heightAnchor()), 250.0),
        ]);
        Retained::into_super(scroll)
    }
}

/// A row's controls side by side; never cut (the grid is held to its least width).
fn row_stack(views: &[&NSView], mtm: MainThreadMarker) -> Retained<NSStackView> {
    let s = layout::hstack(views, mtm);
    s.setClippingResistancePriority_forOrientation(
        1000.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    s
}

/// `v` as a plain view (borrowed).
fn as_view(v: &NSView) -> &NSView {
    v
}

/// `v` as a plain view.
fn into_view(v: &NSView) -> Retained<NSView> {
    v.retain()
}

/// A control's `NSControl`.
fn control_of(ctl: &Ctl) -> Retained<NSControl> {
    match ctl {
        Ctl::Check(b) => Retained::into_super(b.clone()),
        Ctl::Text(t) => Retained::into_super(t.clone()),
        Ctl::Choice(p, _) => Retained::into_super(Retained::into_super(p.clone())),
    }
}

/// A note's text: the Advanced page's says how values apply here (at once, not with OK).
fn note_text(text: String) -> String {
    if text == lang::text(lang::S::PrefAdvancedNote) {
        rookparcel_frontend::tr!(MacPrefAdvancedNote).to_owned()
    } else {
        text
    }
}

/// Scrolls a page (its scroll view) back to the top.
pub(crate) fn scroll_to_top(view: &NSView) {
    let any: &objc2::runtime::AnyObject = view.as_ref();
    let Some(scroll) = any.downcast_ref::<NSScrollView>() else { return };
    let Some(doc) = scroll.documentView() else { return };
    let y = if doc.isFlipped() { 0.0 } else { (doc.bounds().size.height - 1.0).max(0.0) };
    doc.scrollRectToVisible(NSRect::new(NSPoint::new(0.0, y), NSSize::new(1.0, 1.0)));
}
