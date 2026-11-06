//! `RPCanvas`: every drawn view (convention 1), the Done bars in the list, the General tab's
//! bars, the piece map, the block bar, the speed graph and the scheduler grid.
//!
//! A canvas has a [`Kind`] and a value (anything, `Box<dyn Any>`: a fraction, a bitfield, a
//! history); the kind's painter, [`register`]ed once by the slice that owns it, draws the value
//! with [`Painter`]'s helpers in semantic colours (top-left origin: the view is flipped). The
//! canvas redraws when its value changes ([`RPCanvas::set_value`]) and when its effective
//! appearance does (light / dark), and carries an accessibility label and value so VoiceOver
//! reads what it shows. A kind may also take mouse clicks and drags (the scheduler grid).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSEvent, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSResponder, NSStringDrawing, NSTextAlignment,
    NSView,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

/// What a canvas shows; each kind has one painter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    /// The list's Done column: a [`Bar`].
    DoneBar,
    /// General tab: Downloaded and Availability (slice D).
    PieceBar,
    /// Pieces tab: the map (slice D).
    PieceMap,
    /// Pieces tab: the selected piece's blocks (slice D).
    BlockBar,
    /// Speed tab: the 600 s graph (slice D).
    SpeedGraph,
    /// Preferences > Scheduler: the week grid (slice E).
    SchedGrid,
}

/// The Done column's value: the fraction done and the text on the bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bar {
    pub fraction: f64,
    /// Complete: green; downloading: blue.
    pub complete: bool,
    pub text: String,
}

/// A mouse event on a canvas: `Down` then `Drag`s then `Up`, at a point in the view (top-left
/// origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mouse {
    Down(f64, f64),
    Drag(f64, f64),
    Up(f64, f64),
    /// Moved without a button: for hover texts.
    Moved(f64, f64),
}

/// A kind's drawing function: the painter and the canvas's value.
pub type Draw = fn(&Painter, &dyn Any);
/// A kind's mouse handler: the canvas, the event; runs outside drawing.
pub type OnMouse = fn(&RPCanvas, Mouse);

#[derive(Clone, Copy)]
struct Entry {
    draw: Draw,
    mouse: Option<OnMouse>,
}

thread_local! {
    static PAINTERS: RefCell<HashMap<Kind, Entry>> = RefCell::new(HashMap::new());
}

/// Sets `kind`'s painter (and mouse handler); once, before its first canvas draws.
pub fn register(kind: Kind, draw: Draw, mouse: Option<OnMouse>) {
    PAINTERS.with(|p| p.borrow_mut().insert(kind, Entry { draw, mouse }));
}

pub struct Ivars {
    kind: Cell<Kind>,
    value: RefCell<Box<dyn Any>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond calling super's designated
    // initializer (`initWithFrame:` below), and RPCanvas does not implement Drop.
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RPCanvas"]
    #[ivars = Ivars]
    pub struct RPCanvas;

    impl RPCanvas {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let Some(entry) = PAINTERS.with(|p| p.borrow().get(&self.ivars().kind.get()).copied()) else { return };
            let painter = Painter { bounds: self.bounds(), dark: crate::images::is_dark(self) };
            // a painter only reads the value; the borrow ends with the drawing
            let value = self.ivars().value.borrow();
            (entry.draw)(&painter, &**value);
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.mouse(event, Mouse::Down);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.mouse(event, Mouse::Drag);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.mouse(event, Mouse::Up);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.mouse(event, Mouse::Moved);
        }
    }
);

impl RPCanvas {
    /// A canvas of `kind` showing `value`.
    pub fn new(kind: Kind, value: Box<dyn Any>, mtm: MainThreadMarker) -> Retained<RPCanvas> {
        let this =
            RPCanvas::alloc(mtm).set_ivars(Ivars { kind: Cell::new(kind), value: RefCell::new(value) });
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 16.0));
        // SAFETY: NSView's designated initializer, on a fresh object with its ivars set.
        let view: Retained<RPCanvas> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        view
    }

    pub fn kind(&self) -> Kind {
        self.ivars().kind.get()
    }

    /// Shows `value` (redrawn when it differs from the one shown, for the types that compare:
    /// [`Bar`] and [`String`]; always for others).
    pub fn set_value(&self, value: Box<dyn Any>) {
        let same = {
            let old = self.ivars().value.borrow();
            match (old.downcast_ref::<Bar>(), value.downcast_ref::<Bar>()) {
                (Some(a), Some(b)) => a == b,
                _ => matches!(
                    (old.downcast_ref::<String>(), value.downcast_ref::<String>()),
                    (Some(a), Some(b)) if a == b
                ),
            }
        };
        if !same {
            *self.ivars().value.borrow_mut() = value;
            self.setNeedsDisplay(true);
        }
    }

    /// Reads the value (`None` while it is of another type).
    pub fn with_value<T: 'static, R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.ivars().value.borrow().downcast_ref::<T>().map(f)
    }

    /// What VoiceOver says: the label (what it is) and the value (what it shows now).
    pub fn set_accessibility(&self, label: &str, value: &str) {
        let (l, v) = (NSString::from_str(label), NSString::from_str(value));
        // SAFETY: NSView answers the NSAccessibility setters with an NSString (`id` for the
        // value, which takes a string).
        unsafe {
            let _: () = msg_send![self, setAccessibilityElement: true];
            let _: () = msg_send![self, setAccessibilityLabel: &*l];
            let _: () = msg_send![self, setAccessibilityValue: &*v as &AnyObject];
        }
    }

    fn mouse(&self, event: &NSEvent, make: fn(f64, f64) -> Mouse) {
        let Some(handler) = PAINTERS.with(|p| p.borrow().get(&self.kind()).and_then(|e| e.mouse)) else {
            return;
        };
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        handler(self, make(p.x, p.y));
    }
}

/// What a painter draws with: the canvas's bounds (top-left origin) and whether it is dark.
pub struct Painter {
    pub bounds: NSRect,
    pub dark: bool,
}

/// A rectangle from its top-left corner and size.
pub fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(0.0), h.max(0.0)))
}

impl Painter {
    pub fn width(&self) -> f64 {
        self.bounds.size.width
    }

    pub fn height(&self) -> f64 {
        self.bounds.size.height
    }

    /// Fills `r` with `color`.
    pub fn fill(&self, r: NSRect, color: &NSColor) {
        color.setFill();
        NSBezierPath::fillRect(r);
    }

    /// Fills `r` with rounded corners.
    pub fn fill_rounded(&self, r: NSRect, radius: f64, color: &NSColor) {
        color.setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, radius, radius).fill();
    }

    /// A one-point frame inside `r`.
    pub fn frame(&self, r: NSRect, color: &NSColor) {
        color.setStroke();
        let inner = rect(r.origin.x + 0.5, r.origin.y + 0.5, r.size.width - 1.0, r.size.height - 1.0);
        let path = NSBezierPath::bezierPathWithRect(inner);
        path.setLineWidth(1.0);
        path.stroke();
    }

    /// A line.
    pub fn line(&self, from: (f64, f64), to: (f64, f64), width: f64, color: &NSColor) {
        color.setStroke();
        let path = NSBezierPath::bezierPath();
        path.moveToPoint(NSPoint::new(from.0, from.1));
        path.lineToPoint(NSPoint::new(to.0, to.1));
        path.setLineWidth(width);
        path.stroke();
    }

    /// `s` in `r`, vertically centred, at `size` points (0: the small system size).
    pub fn text(&self, s: &str, r: NSRect, align: NSTextAlignment, size: f64, color: &NSColor) {
        let size = if size > 0.0 { size } else { NSFont::smallSystemFontSize() };
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(size, 0.0);
        let style = NSMutableParagraphStyle::new();
        style.setAlignment(align);
        // SAFETY: AppKit's attribute name constants.
        let keys =
            unsafe { [NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName] };
        let values: [&AnyObject; 3] = [font.as_ref(), color.as_ref(), style.as_ref()];
        let attrs = NSDictionary::from_slices(&keys, &values);
        let ns = NSString::from_str(s);
        let line = font.ascender() - font.descender();
        let y = r.origin.y + ((r.size.height - line) / 2.0).max(0.0);
        // SAFETY: the dictionary holds a font, a colour and a paragraph style under their keys.
        unsafe { ns.drawInRect_withAttributes(rect(r.origin.x, y, r.size.width, line), Some(&attrs)) };
    }
}

/// The Done column's bar: blue while downloading, green when complete, the percentage over it.
fn draw_done(p: &Painter, value: &dyn Any) {
    let Some(bar) = value.downcast_ref::<Bar>() else { return };
    let r = rect(1.0, 2.0, p.width() - 2.0, p.height() - 4.0);
    p.fill(r, &NSColor::quaternaryLabelColor());
    let fill = if bar.complete { NSColor::systemGreenColor() } else { NSColor::systemBlueColor() };
    p.fill(rect(r.origin.x, r.origin.y, r.size.width * bar.fraction.clamp(0.0, 1.0), r.size.height), &fill);
    p.frame(r, &NSColor::separatorColor());
    p.text(&bar.text, r, NSTextAlignment::Center, 0.0, &NSColor::labelColor());
}

/// Registers the painters this slice owns (the Done bar).
pub fn init() {
    register(Kind::DoneBar, draw_done, None);
}
