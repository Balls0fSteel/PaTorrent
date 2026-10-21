//! Preferences > Scheduler's table: 7 days × 24 hours, one coloured cell per hour, as in
//! µTorrent 2.2.1 and the Win32 client. A click moves a cell to the next mode; dragging paints
//! the cells passed over with that same mode. The legend and the hovered hour's text are drawn
//! below the cells. In a right-to-left language the hours run from the right.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use patorrent_frontend::lang;
use patorrent_frontend::scheduler::{CELLS, DAY_NAMES, DAYS, HOURS, Mode, drag_line, hover_text};

use crate::canvas::{self, Canvas, faded, fill, frame, rgb};

struct Grid {
    table: [Mode; CELLS],
    /// The mode a drag paints and the cell it last painted, while the button is down.
    painting: Option<(Mode, usize)>,
    /// Where the drag started, in widget coordinates.
    origin: (f64, f64),
    hover: Option<usize>,
}

pub struct SchedGrid {
    pub widget: Canvas,
    grid: Rc<RefCell<Grid>>,
}

/// Where things go in a widget `w` × `h` pixels, in left-to-right coordinates (`x` mirrors them).
struct Layout {
    w: f32,
    rtl: bool,
    label_w: f32,
    top: f32,
    cell_w: f32,
    cell_h: f32,
    legend_y: f32,
    line_h: f32,
}

impl Layout {
    fn new(widget: &gtk::Widget, w: f32, h: f32) -> Layout {
        let text_h = widget.create_pango_layout(Some("Mg")).pixel_size().1 as f32;
        let line_h = (text_h + 6.0).max(14.0);
        let label_w = DAY_NAMES
            .iter()
            .map(|d| widget.create_pango_layout(Some(lang::text(*d))).pixel_size().0 as f32)
            .fold(0.0, f32::max)
            + 10.0;
        let cell_w = ((w - label_w) / HOURS as f32).floor().max(2.0);
        let top = line_h;
        let cell_h = ((h - 3.0 * line_h - 4.0) / DAYS as f32).floor().max(4.0);
        Layout {
            w,
            rtl: widget.direction() == gtk::TextDirection::Rtl,
            label_w,
            top,
            cell_w,
            cell_h,
            legend_y: top + cell_h * DAYS as f32 + 4.0,
            line_h,
        }
    }

    /// The left edge on screen of a box `width` wide whose left-to-right left edge is `x`.
    fn x(&self, x: f32, width: f32) -> f32 {
        if self.rtl { self.w - x - width } else { x }
    }

    /// A cell's left-to-right position and size (one pixel wider and taller, so frames share
    /// their lines).
    fn cell(&self, day: usize, hour: usize) -> (f32, f32, f32, f32) {
        let x = self.label_w + hour as f32 * self.cell_w;
        (x, self.top + day as f32 * self.cell_h, self.cell_w + 1.0, self.cell_h + 1.0)
    }

    fn hit(&self, x: f64, y: f64) -> Option<usize> {
        let x = if self.rtl { self.w - x as f32 } else { x as f32 };
        let y = y as f32;
        if x < self.label_w || y < self.top {
            return None;
        }
        let (hour, day) = ((x - self.label_w) / self.cell_w, (y - self.top) / self.cell_h);
        ((hour as usize) < HOURS && (day as usize) < DAYS).then(|| day as usize * HOURS + hour as usize)
    }
}

impl SchedGrid {
    pub fn new() -> SchedGrid {
        let grid = Rc::new(RefCell::new(Grid {
            table: [Mode::Full; CELLS],
            painting: None,
            origin: (0.0, 0.0),
            hover: None,
        }));
        let g = grid.clone();
        let widget = Canvas::new(move |w, s, width, height| paint(w, s, width, height, &g.borrow()));
        widget.set_size_request(420, 200);
        widget.set_hexpand(true);
        // the cells fade with the rest of the page while the scheduler is off
        widget.connect_state_flags_changed(|w, _| w.queue_draw());

        let drag = gtk::GestureDrag::new();
        drag.set_button(gtk::gdk::BUTTON_PRIMARY);
        let g = Rc::downgrade(&grid);
        drag.connect_drag_begin(move |d, x, y| {
            let (Some(g), Some(w)) = (g.upgrade(), d.widget()) else { return };
            let Some(cell) = layout(&w).hit(x, y) else { return };
            let mut g = g.borrow_mut();
            let m = g.table[cell].next();
            g.table[cell] = m;
            g.painting = Some((m, cell));
            g.origin = (x, y);
            g.hover = Some(cell);
            w.queue_draw();
        });
        let g = Rc::downgrade(&grid);
        drag.connect_drag_update(move |d, dx, dy| {
            let (Some(g), Some(w)) = (g.upgrade(), d.widget()) else { return };
            let mut g = g.borrow_mut();
            let Some((m, from)) = g.painting else { return };
            let (x, y) = (g.origin.0 + dx, g.origin.1 + dy);
            let cell = layout(&w).hit(x, y);
            let mut dirty = cell != g.hover;
            g.hover = cell;
            if let Some(c) = cell {
                for i in drag_line(from, c) {
                    dirty |= g.table[i] != m;
                    g.table[i] = m;
                }
                g.painting = Some((m, c));
            }
            if dirty {
                w.queue_draw();
            }
        });
        let g = Rc::downgrade(&grid);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(g) = g.upgrade() {
                g.borrow_mut().painting = None;
            }
        });
        widget.add_controller(drag);

        let motion = gtk::EventControllerMotion::new();
        let g = Rc::downgrade(&grid);
        motion.connect_motion(move |m, x, y| {
            let (Some(g), Some(w)) = (g.upgrade(), m.widget()) else { return };
            let cell = layout(&w).hit(x, y);
            let mut g = g.borrow_mut();
            if g.painting.is_none() && cell != g.hover {
                g.hover = cell;
                w.queue_draw();
            }
        });
        widget.add_controller(motion);
        SchedGrid { widget, grid }
    }

    /// Shows `table`.
    pub fn set(&self, table: &[Mode; CELLS]) {
        self.grid.borrow_mut().table = *table;
        self.widget.queue_draw();
    }

    /// The table as edited.
    pub fn table(&self) -> [Mode; CELLS] {
        self.grid.borrow().table
    }
}

fn layout(w: &gtk::Widget) -> Layout {
    Layout::new(w, w.width() as f32, w.height() as f32)
}

fn paint(w: &gtk::Widget, s: &gtk::Snapshot, width: f32, height: f32, g: &Grid) {
    let l = Layout::new(w, width, height);
    let fg = w.color();
    let line = faded(w, 0.5);
    // the cells keep µTorrent's colours in either theme; greyed, they fade
    let alpha = if w.is_sensitive() { 1.0 } else { 0.45 };
    let color = |m: Mode| {
        let c = rgb(m.rgb());
        gtk::gdk::RGBA::new(c.red(), c.green(), c.blue(), alpha)
    };
    let text = |t: &str, x: f32, y: f32, centre_w: Option<f32>| {
        let tw = w.create_pango_layout(Some(t)).pixel_size().0 as f32;
        let x = match centre_w {
            Some(cw) => l.x(x, cw) + (cw - tw) / 2.0,
            None => l.x(x, tw),
        };
        canvas::text(w, s, t, (x, y, l.line_h), false, &fg);
    };
    for h in (0..HOURS).step_by(3) {
        let (x, ..) = l.cell(0, h);
        text(&h.to_string(), x - l.cell_w, 0.0, Some(3.0 * l.cell_w));
    }
    for (day, name) in DAY_NAMES.iter().enumerate() {
        let (_, y, _, ch) = l.cell(day, 0);
        text(lang::text(*name), 0.0, y + (ch - l.line_h) / 2.0, None);
        for hour in 0..HOURS {
            let (x, y, cw, ch) = l.cell(day, hour);
            let x = l.x(x, cw);
            fill(s, x, y, cw, ch, &color(g.table[day * HOURS + hour]));
            frame(s, x, y, cw, ch, &line);
        }
    }
    // the legend: a swatch and the mode's name, four to the line in equal slots; when a
    // translation does not fit its slot, packed from the edge instead
    let sw = l.line_h - 4.0;
    let widths = Mode::ALL.map(|m| sw + 4.0 + w.create_pango_layout(Some(m.label())).pixel_size().0 as f32);
    let n = Mode::ALL.len() as f32;
    let slot = (width - l.label_w) / n;
    let packed = widths.iter().any(|&x| x > slot);
    let gap = if packed { ((width - widths.iter().sum::<f32>()) / (n - 1.0)).max(4.0) } else { 0.0 };
    let mut x = if packed { 0.0 } else { l.label_w };
    for (m, mw) in Mode::ALL.into_iter().zip(widths) {
        let sx = l.x(x, sw);
        fill(s, sx, l.legend_y + 2.0, sw, sw, &color(m));
        frame(s, sx, l.legend_y + 2.0, sw, sw, &line);
        text(m.label(), x + sw + 4.0, l.legend_y, None);
        x += if packed { mw + gap } else { slot };
    }
    if let Some(cell) = g.hover {
        text(&hover_text(&g.table, cell), l.label_w, l.legend_y + l.line_h, None);
    }
}
