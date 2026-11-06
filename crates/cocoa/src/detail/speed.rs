//! The Speed tab: the session's download and upload rates over the last ten minutes (600
//! samples, newest at the right), under a legend with the current rates, on a scale of 1, 2 or
//! 5 × 10ⁿ kB/s (`detail::speed_scale`) with four grid lines. Redrawn each second.

use std::any::Any;
use std::collections::VecDeque;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSBezierPath, NSColor, NSLineJoinStyle, NSTextAlignment};
use objc2_foundation::NSPoint;
use rookparcel_frontend::app::drawn;
use rookparcel_frontend::detail::{Tab, speed_scale};
use rookparcel_frontend::{fmt, lang, tr};

use crate::app::peek;
use crate::canvas::{self, Kind, Painter, RPCanvas, rect};
use crate::ui;

/// The graph's value: the samples, oldest first.
#[derive(Default)]
pub struct Speed {
    pub samples: VecDeque<(u32, u32)>,
}

/// The legend's texts (also what VoiceOver reads).
fn legend(s: &VecDeque<(u32, u32)>) -> [String; 2] {
    let (down, up) = s.back().copied().unwrap_or((0, 0));
    [tr!(SpeedDownload, rate = fmt::speed_always(down)), tr!(SpeedUpload, rate = fmt::speed_always(up))]
}

fn colors() -> [Retained<NSColor>; 2] {
    [NSColor::systemGreenColor(), NSColor::systemRedColor()]
}

fn draw(p: &Painter, value: &dyn Any) {
    let Some(speed) = value.downcast_ref::<Speed>() else { return };
    let (pad, legend_h, axis, line) = (8.0, 22.0, 76.0, 16.0);
    let (left, top) = (pad + axis, pad + legend_h);
    let (pw, ph) = (p.width() - left - pad, p.height() - top - pad);
    if pw < 20.0 || ph < 20.0 {
        return;
    }
    let text = NSColor::labelColor();
    let scale = speed_scale(&speed.samples);

    // the legend
    let mut x = left;
    for (t, color) in legend(&speed.samples).iter().zip(colors()) {
        p.fill(rect(x, pad + 3.0, 10.0, 10.0), &color);
        p.text(t, rect(x + 14.0, pad, 220.0, line), NSTextAlignment::Left, 0.0, &text);
        x += 240.0;
    }

    // the grid and the scale
    for i in 0..=4 {
        let y = top + ph * f64::from(i) / 4.0;
        if i > 0 && i < 4 {
            p.fill(rect(left + 1.0, y.floor(), pw - 2.0, 1.0), &NSColor::separatorColor());
        }
        if i < 4 {
            let v = scale * (4 - i) as u64 / 4;
            let label = fmt::speed_always(v as u32);
            p.text(&label, rect(pad, y - line / 2.0, axis - 4.0, line), NSTextAlignment::Right, 0.0, &text);
        }
    }
    p.frame(rect(left, top, pw, ph), &NSColor::tertiaryLabelColor());

    // the two rates
    for (points, color) in drawn::speed_lines(&speed.samples, scale, pw, ph).iter().zip(colors()) {
        let Some(&(x0, y0)) = points.first() else { continue };
        let path = NSBezierPath::bezierPath();
        path.moveToPoint(NSPoint::new(left + x0, top + y0));
        for &(x, y) in &points[1..] {
            path.lineToPoint(NSPoint::new(left + x, top + y));
        }
        path.setLineWidth(2.0);
        path.setLineJoinStyle(NSLineJoinStyle::Round);
        color.setStroke();
        path.stroke();
    }
}

/// The graph.
pub fn new(mtm: MainThreadMarker) -> Retained<RPCanvas> {
    RPCanvas::new(Kind::SpeedGraph, Box::new(Speed::default()), mtm)
}

/// Shows the samples `Detail` has now.
pub fn refresh(graph: &RPCanvas) {
    let Some(samples) = peek(|c| c.detail().speed.clone()) else { return };
    let [down, up] = legend(&samples);
    graph.set_accessibility(&ui::text(lang::text(Tab::Speed.title())), &format!("{down}, {up}"));
    graph.set_value(Box::new(Speed { samples }));
}

/// Registers the tab's painter.
pub fn register() {
    canvas::register(Kind::SpeedGraph, draw, None);
}
