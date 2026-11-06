//! Preferences > Scheduler's table on `RPCanvas` (`Kind::SchedGrid`): 7 days × 24 hours, one
//! coloured cell per hour, as in µTorrent 2.2.1 and the other clients (GTK `sched_grid.rs`). A
//! click moves a cell to the next mode; dragging paints the cells passed over with that mode
//! (`scheduler::drag_line`); letting go applies the table. The legend and the hovered hour's
//! text (`scheduler::hover_text`) are drawn under the cells. In a right-to-left language the
//! hours run from the right. Greyed (the scheduler off), the cells fade and take no clicks.
//!
//! The canvas's value is the [`Grid`]; the mouse handler changes a copy and sets it back.

use std::any::Any;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSStringDrawing, NSTextAlignment, NSTrackingArea,
    NSTrackingAreaOptions, NSUserInterfaceLayoutDirection,
};
use objc2_foundation::{NSDictionary, NSString};
use rookparcel_frontend::lang::{self, S};
use rookparcel_frontend::scheduler::{CELLS, DAY_NAMES, DAYS, HOURS, Mode, drag_line, hover_text};

use crate::canvas::{self, Kind, Mouse, Painter, RPCanvas, rect};
use crate::ui::layout;

/// What the canvas shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Grid {
    pub table: [Mode; CELLS],
    /// The mode a drag paints and the cell it painted last, while the button is down.
    painting: Option<(Mode, usize)>,
    hover: Option<usize>,
    enabled: bool,
    rtl: bool,
}

/// A new grid canvas (its painter registered).
pub(crate) fn build(mtm: MainThreadMarker) -> Retained<RPCanvas> {
    canvas::register(Kind::SchedGrid, draw, Some(mouse));
    let grid = Grid { table: [Mode::Full; CELLS], painting: None, hover: None, enabled: true, rtl: false };
    let c = RPCanvas::new(Kind::SchedGrid, Box::new(grid), mtm);
    layout::height(&c, 214.0);
    layout::min_width(&c, 500.0);
    layout::stretch(&c);
    // hover texts: mouse moves while the window is key
    let options = NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::ActiveInKeyWindow
        | NSTrackingAreaOptions::InVisibleRect;
    let owner: &AnyObject = c.as_ref();
    // SAFETY: the canvas owns the area (it answers `mouseMoved:`); the rect is ignored with
    // `InVisibleRect`; no user info.
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            rect(0.0, 0.0, 0.0, 0.0),
            options,
            Some(owner),
            None,
        )
    };
    c.addTrackingArea(&area);
    c.set_accessibility(&crate::ui::text(lang::text(S::PrefSchedTable)), "");
    c
}

fn grid_of(c: &RPCanvas) -> Option<Grid> {
    c.with_value::<Grid, _>(Clone::clone)
}

fn set(c: &RPCanvas, g: Grid) {
    let hover = g.hover.map(|cell| hover_text(&g.table, cell)).unwrap_or_default();
    c.set_value(Box::new(g));
    c.set_accessibility(&crate::ui::text(lang::text(S::PrefSchedTable)), &hover);
}

/// Shows `table` (as applied).
pub(crate) fn set_table(c: &RPCanvas, table: [Mode; CELLS]) {
    let Some(mut g) = grid_of(c) else { return };
    if g.table != table && g.painting.is_none() {
        g.table = table;
        set(c, g);
    }
}

/// Greys the grid (the scheduler is off) or not.
pub(crate) fn set_enabled(c: &RPCanvas, on: bool) {
    let Some(mut g) = grid_of(c) else { return };
    let rtl = c.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft;
    if g.enabled != on || g.rtl != rtl {
        g.enabled = on;
        g.rtl = rtl;
        g.painting = None;
        set(c, g);
    }
}

/// The font the painter writes with (`Painter::text`).
fn font() -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(NSFont::smallSystemFontSize(), 0.0)
}

/// A text's width in that font.
fn text_width(s: &str) -> f64 {
    let font = font();
    // SAFETY: AppKit's attribute name constant.
    let keys = unsafe { [NSFontAttributeName] };
    let values: [&AnyObject; 1] = [font.as_ref()];
    let attrs = NSDictionary::from_slices(&keys, &values);
    // SAFETY: the dictionary holds a font under its key.
    unsafe { NSString::from_str(s).sizeWithAttributes(Some(&attrs)) }.width
}

/// Where things go in a canvas `w` × `h` points, in left-to-right coordinates (`x` mirrors).
struct Layout {
    w: f64,
    rtl: bool,
    label_w: f64,
    top: f64,
    cell_w: f64,
    cell_h: f64,
    legend_y: f64,
    line_h: f64,
}

impl Layout {
    fn new(w: f64, h: f64, rtl: bool) -> Layout {
        let f = font();
        let line_h = (f.ascender() - f.descender() + 6.0).max(14.0);
        let label_w = DAY_NAMES.iter().map(|d| text_width(lang::text(*d))).fold(0.0, f64::max) + 10.0;
        let cell_w = ((w - label_w) / HOURS as f64).floor().max(2.0);
        let top = line_h;
        let cell_h = ((h - 3.0 * line_h - 4.0) / DAYS as f64).floor().max(4.0);
        Layout { w, rtl, label_w, top, cell_w, cell_h, legend_y: top + cell_h * DAYS as f64 + 4.0, line_h }
    }

    /// The left edge on screen of a box `width` wide whose left-to-right left edge is `x`.
    fn x(&self, x: f64, width: f64) -> f64 {
        if self.rtl { self.w - x - width } else { x }
    }

    /// A cell's left-to-right position and size.
    fn cell(&self, day: usize, hour: usize) -> (f64, f64, f64, f64) {
        (
            self.label_w + hour as f64 * self.cell_w,
            self.top + day as f64 * self.cell_h,
            self.cell_w,
            self.cell_h,
        )
    }

    fn hit(&self, x: f64, y: f64) -> Option<usize> {
        let x = if self.rtl { self.w - x } else { x };
        if x < self.label_w || y < self.top {
            return None;
        }
        let (hour, day) =
            (((x - self.label_w) / self.cell_w) as usize, ((y - self.top) / self.cell_h) as usize);
        (hour < HOURS && day < DAYS).then_some(day * HOURS + hour)
    }
}

/// A mode's colour (µTorrent's, in either appearance), faded while greyed.
fn color(m: Mode, alpha: f64) -> Retained<NSColor> {
    let c = m.rgb();
    let part = |shift: u32| f64::from((c >> shift) & 0xff) / 255.0;
    NSColor::colorWithSRGBRed_green_blue_alpha(part(16), part(8), part(0), alpha)
}

fn draw(p: &Painter, value: &dyn Any) {
    let Some(g) = value.downcast_ref::<Grid>() else { return };
    let l = Layout::new(p.width(), p.height(), g.rtl);
    let alpha = if g.enabled { 1.0 } else { 0.45 };
    let line = NSColor::tertiaryLabelColor();
    let ink = if g.enabled { NSColor::labelColor() } else { NSColor::disabledControlTextColor() };
    let natural = if g.rtl { NSTextAlignment::Right } else { NSTextAlignment::Left };
    // the hours, every third over its cell
    for h in (0..HOURS).step_by(3) {
        let (x, ..) = l.cell(0, h);
        let w = 3.0 * l.cell_w;
        p.text(
            &h.to_string(),
            rect(l.x(x - l.cell_w, w), 0.0, w, l.line_h),
            NSTextAlignment::Center,
            0.0,
            &ink,
        );
    }
    for (day, name) in DAY_NAMES.iter().enumerate() {
        let (_, y, _, ch) = l.cell(day, 0);
        let lw = l.label_w - 6.0;
        p.text(lang::text(*name), rect(l.x(0.0, lw), y, lw, ch), natural, 0.0, &ink);
        for hour in 0..HOURS {
            let (x, y, cw, ch) = l.cell(day, hour);
            // one point bigger, so neighbours share their frame lines
            let r = rect(l.x(x, cw + 1.0), y, cw + 1.0, ch + 1.0);
            p.fill(r, &color(g.table[day * HOURS + hour], alpha));
            p.frame(r, &line);
        }
    }
    // the legend: a swatch and the mode's name, four in equal slots, or packed from the edge
    // when a translation does not fit its slot
    let sw = l.line_h - 4.0;
    let widths = Mode::ALL.map(|m| sw + 4.0 + text_width(m.label()));
    let n = Mode::ALL.len() as f64;
    let slot = (l.w - l.label_w) / n;
    let packed = widths.iter().any(|&x| x > slot);
    let gap = if packed { ((l.w - widths.iter().sum::<f64>()) / (n - 1.0)).max(4.0) } else { 0.0 };
    let mut x = if packed { 0.0 } else { l.label_w };
    for (m, mw) in Mode::ALL.into_iter().zip(widths) {
        let r = rect(l.x(x, sw), l.legend_y + 2.0, sw, sw);
        p.fill(r, &color(m, alpha));
        p.frame(r, &line);
        let tw = mw - sw - 4.0;
        p.text(m.label(), rect(l.x(x + sw + 4.0, tw), l.legend_y, tw, l.line_h), natural, 0.0, &ink);
        x += if packed { mw + gap } else { slot };
    }
    if let Some(cell) = g.hover {
        let w = l.w - l.label_w;
        p.text(
            &hover_text(&g.table, cell),
            rect(l.x(l.label_w, w), l.legend_y + l.line_h, w, l.line_h),
            natural,
            0.0,
            &NSColor::labelColor(),
        );
    }
}

pub(super) fn mouse(c: &RPCanvas, m: Mouse) {
    let Some(mut g) = grid_of(c) else { return };
    let b = c.bounds();
    let l = Layout::new(b.size.width, b.size.height, g.rtl);
    let before = g.clone();
    let mut done = false;
    match m {
        Mouse::Down(x, y) if g.enabled => {
            if let Some(cell) = l.hit(x, y) {
                let mode = g.table[cell].next();
                g.table[cell] = mode;
                g.painting = Some((mode, cell));
                g.hover = Some(cell);
            }
        }
        Mouse::Drag(x, y) => {
            let cell = l.hit(x, y);
            g.hover = cell;
            if let (Some((mode, from)), Some(to)) = (g.painting, cell) {
                for i in drag_line(from, to) {
                    g.table[i] = mode;
                }
                g.painting = Some((mode, to));
            }
        }
        Mouse::Up(..) => done = g.painting.take().is_some(),
        Mouse::Moved(x, y) => g.hover = l.hit(x, y),
        Mouse::Down(..) => {}
    }
    let table = g.table;
    if g != before {
        set(c, g);
    }
    if done {
        super::schedule_changed(table);
    }
}
