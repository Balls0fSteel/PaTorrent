//! A widget drawn by a callback into its `GtkSnapshot`: the General tab's bars, the piece map,
//! the Pieces list's Blocks cells and the Speed graph. Text goes through Pango layouts (so it
//! follows the theme's font and the text direction), lines through Cairo.
//!
//! A canvas can ask for a height that depends on its width (the piece map's rows of squares).

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};

/// Draws the canvas: the widget (for its colours and Pango context), the snapshot, the size.
pub type Paint = Box<dyn Fn(&gtk::Widget, &gtk::Snapshot, f32, f32)>;

/// The height wanted for a width.
pub type HeightForWidth = Box<dyn Fn(i32) -> i32>;

mod imp {
    use std::cell::RefCell;

    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct Canvas {
        pub paint: RefCell<Option<super::Paint>>,
        pub height: RefCell<Option<super::HeightForWidth>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "PaTorrentCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Canvas {}

    impl WidgetImpl for Canvas {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            if self.height.borrow().is_some() {
                gtk::SizeRequestMode::HeightForWidth
            } else {
                gtk::SizeRequestMode::ConstantSize
            }
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Vertical {
                if let Some(h) = &*self.height.borrow() {
                    // no width yet: as if it were a narrow one
                    let h = h(if for_size > 0 { for_size } else { 300 });
                    return (h, h, -1, -1);
                }
            }
            self.parent_measure(orientation, for_size)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            if let Some(paint) = &*self.paint.borrow() {
                paint(obj.upcast_ref(), snapshot, w, h);
            }
        }
    }
}

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Canvas {
    pub fn new(paint: impl Fn(&gtk::Widget, &gtk::Snapshot, f32, f32) + 'static) -> Canvas {
        let c: Canvas = glib::Object::new();
        *c.imp().paint.borrow_mut() = Some(Box::new(paint));
        c
    }

    pub fn set_height_for_width(&self, f: impl Fn(i32) -> i32 + 'static) {
        *self.imp().height.borrow_mut() = Some(Box::new(f));
        self.queue_resize();
    }
}

/// `0xRRGGBB` as a colour.
pub fn rgb(c: u32) -> gdk::RGBA {
    let ch = |shift: u32| ((c >> shift) & 0xff) as f32 / 255.0;
    gdk::RGBA::new(ch(16), ch(8), ch(0), 1.0)
}

/// The widget's text colour at `alpha` (grid lines, frames, hints in either theme).
pub fn faded(widget: &gtk::Widget, alpha: f32) -> gdk::RGBA {
    let c = widget.color();
    gdk::RGBA::new(c.red(), c.green(), c.blue(), c.alpha() * alpha)
}

pub fn fill(s: &gtk::Snapshot, x: f32, y: f32, w: f32, h: f32, color: &gdk::RGBA) {
    if w > 0.0 && h > 0.0 {
        s.append_color(color, &graphene::Rect::new(x, y, w, h));
    }
}

/// A one-pixel frame just inside the rectangle.
pub fn frame(s: &gtk::Snapshot, x: f32, y: f32, w: f32, h: f32, color: &gdk::RGBA) {
    fill(s, x, y, w, 1.0, color);
    fill(s, x, y + h - 1.0, w, 1.0, color);
    fill(s, x, y, 1.0, h, color);
    fill(s, x + w - 1.0, y, 1.0, h, color);
}

/// Draws `text` with its top left (or, `right`, its top right) at (x, y), vertically centred on a
/// line `line` high; returns its width.
pub fn text(
    widget: &gtk::Widget,
    s: &gtk::Snapshot,
    text: &str,
    (x, y, line): (f32, f32, f32),
    right: bool,
    color: &gdk::RGBA,
) -> f32 {
    let layout = widget.create_pango_layout(Some(text));
    let (w, h) = layout.pixel_size();
    let x = if right { x - w as f32 } else { x };
    s.save();
    s.translate(&graphene::Point::new(x, y + (line - h as f32) / 2.0));
    s.append_layout(&layout, color);
    s.restore();
    w as f32
}

/// A horizontal bar with one step per piece: `value(i)` in 0..=1 is the fill strength of piece
/// `i` (the Win32 client's `piece_bar`).
pub fn piece_bar(
    widget: &gtk::Widget,
    s: &gtk::Snapshot,
    (x, y, w, h): (f32, f32, f32, f32),
    pieces: usize,
    color: u32,
    value: impl Fn(usize) -> f32,
) {
    fill(s, x, y, w, h, &rgb(0xffffff));
    frame(s, x, y, w, h, &faded(widget, 0.45));
    let inner = (w - 2.0).max(0.0) as usize;
    if pieces == 0 || inner == 0 {
        return;
    }
    let step = |col: usize| {
        let a = col * pieces / inner;
        let b = ((col + 1) * pieces / inner).max(a + 1).min(pieces);
        (a..b).map(&value).sum::<f32>() / (b - a) as f32
    };
    let mut col = 0;
    while col < inner {
        let v = step(col);
        // one rectangle per run of the same strength
        let mut end = col + 1;
        while end < inner && (step(end) - v).abs() <= 0.02 {
            end += 1;
        }
        if v > 0.0 {
            let mix = |shift: u32| {
                let ch = ((color >> shift) & 0xff) as f32;
                ((255.0 - (255.0 - ch) * v.min(1.0)) as u32) << shift
            };
            let c = rgb(mix(16) | mix(8) | mix(0));
            fill(s, x + 1.0 + col as f32, y + 1.0, (end - col) as f32, h - 2.0, &c);
        }
        col = end;
    }
}
