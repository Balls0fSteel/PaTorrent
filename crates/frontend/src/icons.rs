//! Original toolbar, window and tray icons, drawn at run time at the size needed, shared by the
//! GUIs (the Win32 client's toolbar, window and tray icons; the GTK client's tray icon).
//!
//! Each icon is a few vector shapes on a 24-unit grid, rasterised with 4×4 supersampling. No
//! image files are involved, and the icons are sharp at every scale. The GUIs take the pixels
//! in their system's layout ([`toolbar_bgra`], [`app_bgra`], [`app_argb`]).

/// Toolbar images, in image-list order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    AddTorrent,
    AddUrl,
    Create,
    Remove,
    Start,
    Pause,
    Stop,
    Up,
    Down,
    Preferences,
    Rss,
}

impl Icon {
    pub const ALL: [Icon; 11] = [
        Icon::AddTorrent,
        Icon::AddUrl,
        Icon::Create,
        Icon::Remove,
        Icon::Start,
        Icon::Pause,
        Icon::Stop,
        Icon::Up,
        Icon::Down,
        Icon::Preferences,
        Icon::Rss,
    ];
}

#[derive(Clone, Copy)]
enum Paint {
    Solid(u32),
    /// Top to bottom.
    Gradient(u32, u32),
}

/// Premultiplied RGBA, `size` × `size`, in 24-unit design coordinates.
struct Canvas {
    size: usize,
    px: Vec<[f32; 4]>,
}

const SS: usize = 4;

impl Canvas {
    fn new(size: usize) -> Canvas {
        Canvas { size, px: vec![[0.0; 4]; size * size] }
    }

    /// Fills every pixel whose samples satisfy `inside` (design coordinates).
    fn fill(&mut self, paint: Paint, inside: impl Fn(f32, f32) -> bool) {
        let k = 24.0 / self.size as f32;
        for y in 0..self.size {
            for x in 0..self.size {
                let mut hits = 0;
                for sy in 0..SS {
                    for sx in 0..SS {
                        let fx = (x as f32 + (sx as f32 + 0.5) / SS as f32) * k;
                        let fy = (y as f32 + (sy as f32 + 0.5) / SS as f32) * k;
                        if inside(fx, fy) {
                            hits += 1;
                        }
                    }
                }
                if hits == 0 {
                    continue;
                }
                let cov = hits as f32 / (SS * SS) as f32;
                let c = match paint {
                    Paint::Solid(c) => rgb(c),
                    Paint::Gradient(a, b) => {
                        let t = y as f32 / (self.size - 1).max(1) as f32;
                        let (a, b) = (rgb(a), rgb(b));
                        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
                    }
                };
                let p = &mut self.px[y * self.size + x];
                // Source over, premultiplied.
                for i in 0..3 {
                    p[i] = c[i] * cov + p[i] * (1.0 - cov);
                }
                p[3] = cov + p[3] * (1.0 - cov);
            }
        }
    }

    fn poly(&mut self, paint: Paint, pts: &[(f32, f32)]) {
        let pts = pts.to_vec();
        self.fill(paint, move |x, y| in_polygon(&pts, x, y));
    }

    fn rect(&mut self, paint: Paint, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.fill(paint, move |x, y| x >= x0 && x < x1 && y >= y0 && y < y1);
    }

    fn circle(&mut self, paint: Paint, cx: f32, cy: f32, r: f32) {
        self.fill(paint, move |x, y| (x - cx).powi(2) + (y - cy).powi(2) <= r * r);
    }

    fn ring(&mut self, paint: Paint, cx: f32, cy: f32, r_out: f32, r_in: f32) {
        self.fill(paint, move |x, y| {
            let d = (x - cx).powi(2) + (y - cy).powi(2);
            d <= r_out * r_out && d >= r_in * r_in
        });
    }

    /// A stroke with round caps.
    fn line(&mut self, paint: Paint, (x0, y0): (f32, f32), (x1, y1): (f32, f32), w: f32) {
        let r = w / 2.0;
        self.fill(paint, move |x, y| dist_to_segment(x, y, x0, y0, x1, y1) <= r);
    }

    /// Outline of a closed polygon.
    fn outline(&mut self, paint: Paint, pts: &[(f32, f32)], w: f32) {
        let pts = pts.to_vec();
        let r = w / 2.0;
        self.fill(paint, move |x, y| {
            (0..pts.len()).any(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                dist_to_segment(x, y, a.0, a.1, b.0, b.1) <= r
            })
        });
    }

    /// BGRA bytes, premultiplied, top-down.
    fn bgra(&self, gray: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            let (mut r, mut g, mut b, a) = (p[0], p[1], p[2], p[3]);
            if gray {
                // Disabled look: luminance, lighter, half opacity.
                let l = (0.3 * r + 0.59 * g + 0.11 * b) * 0.6 + 0.35 * a;
                (r, g, b) = (l * 0.5, l * 0.5, l * 0.5);
                out.extend([to8(b), to8(g), to8(r), to8(a * 0.5)]);
                continue;
            }
            out.extend([to8(b), to8(g), to8(r), to8(a)]);
        }
        out
    }
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn rgb(c: u32) -> [f32; 3] {
    [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0]
}

fn in_polygon(pts: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn dist_to_segment(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0) };
    ((px - x0 - t * dx).powi(2) + (py - y0 - t * dy).powi(2)).sqrt()
}

const OUTLINE: u32 = 0x3a3a3a;

/// A page with a folded corner, the base of the "add" icons.
fn document(c: &mut Canvas) {
    let page = [(4.0, 2.0), (14.0, 2.0), (19.0, 7.0), (19.0, 22.0), (4.0, 22.0)];
    c.poly(Paint::Gradient(0xffffff, 0xdfe4ea), &page);
    c.outline(Paint::Solid(0x6b7380), &page, 1.0);
    c.poly(Paint::Solid(0xc8ced6), &[(14.0, 2.0), (14.0, 7.0), (19.0, 7.0)]);
    for y in [10.0, 13.0, 16.0] {
        c.line(Paint::Solid(0xa9b1bc), (7.0, y), (13.0, y), 1.0);
    }
}

fn draw(icon: Icon, c: &mut Canvas) {
    let green = Paint::Gradient(0x5fd35f, 0x1f8f2a);
    let blue = Paint::Gradient(0x6fb6ff, 0x1c64c8);
    let red = Paint::Gradient(0xff7a6b, 0xc62a1d);
    match icon {
        Icon::AddTorrent => {
            document(c);
            c.circle(Paint::Solid(0x15651d), 16.5, 16.5, 6.5);
            c.circle(green, 16.5, 16.5, 5.5);
            c.line(Paint::Solid(0xffffff), (16.5, 13.0), (16.5, 20.0), 2.2);
            c.line(Paint::Solid(0xffffff), (13.0, 16.5), (20.0, 16.5), 2.2);
        }
        Icon::AddUrl => {
            document(c);
            c.circle(Paint::Solid(0x174f9c), 16.5, 16.5, 6.5);
            c.circle(blue, 16.5, 16.5, 5.5);
            let white = Paint::Solid(0xffffff);
            c.ring(white, 16.5, 16.5, 4.6, 3.8);
            c.line(white, (16.5, 12.0), (16.5, 21.0), 0.9);
            c.line(white, (12.0, 16.5), (21.0, 16.5), 0.9);
            c.fill(white, |x, y| {
                let d = ((x - 16.5) / 2.2).powi(2) + ((y - 16.5) / 4.6).powi(2);
                (0.72..=1.0).contains(&d)
            });
        }
        Icon::Create => {
            document(c);
            let star: Vec<(f32, f32)> = (0..10)
                .map(|i| {
                    let a = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
                    let r = if i % 2 == 0 { 6.5 } else { 2.8 };
                    (16.5 + r * a.cos(), 16.5 + r * a.sin())
                })
                .collect();
            c.poly(Paint::Gradient(0xffe680, 0xf0a800), &star);
            c.outline(Paint::Solid(0xa86f00), &star, 0.9);
        }
        Icon::Remove => {
            c.line(Paint::Solid(0x8f1d14), (5.0, 5.0), (19.0, 19.0), 5.6);
            c.line(Paint::Solid(0x8f1d14), (19.0, 5.0), (5.0, 19.0), 5.6);
            c.line(red, (5.0, 5.0), (19.0, 19.0), 4.0);
            c.line(red, (19.0, 5.0), (5.0, 19.0), 4.0);
        }
        Icon::Start => {
            let tri = [(6.0, 3.5), (20.5, 12.0), (6.0, 20.5)];
            c.poly(green, &tri);
            c.outline(Paint::Solid(0x15651d), &tri, 1.2);
        }
        Icon::Pause => {
            for x in [5.5, 13.5] {
                c.rect(Paint::Solid(0x2d5f9a), x, 3.5, x + 5.0, 20.5);
                c.rect(blue, x + 0.8, 4.3, x + 4.2, 19.7);
            }
        }
        Icon::Stop => {
            c.rect(Paint::Solid(0x8f1d14), 4.5, 4.5, 19.5, 19.5);
            c.rect(red, 5.5, 5.5, 18.5, 18.5);
        }
        Icon::Up | Icon::Down => {
            let mut arrow = [
                (12.0, 3.0),
                (21.0, 12.5),
                (15.5, 12.5),
                (15.5, 21.0),
                (8.5, 21.0),
                (8.5, 12.5),
                (3.0, 12.5),
            ];
            if icon == Icon::Down {
                for p in &mut arrow {
                    p.1 = 24.0 - p.1;
                }
            }
            c.poly(blue, &arrow);
            c.outline(Paint::Solid(0x174f9c), &arrow, 1.1);
        }
        Icon::Preferences => {
            let teeth = 8;
            c.fill(Paint::Gradient(0xb8c0cc, 0x6b7380), move |x, y| {
                let (dx, dy) = (x - 12.0, y - 12.0);
                let r = (dx * dx + dy * dy).sqrt();
                let a = dy.atan2(dx);
                let tooth =
                    ((a * teeth as f32 / (2.0 * std::f32::consts::PI)).rem_euclid(1.0) - 0.5).abs() < 0.22;
                (r <= 7.5 || (tooth && r <= 10.0)) && r >= 3.2
            });
            c.ring(Paint::Solid(OUTLINE), 12.0, 12.0, 3.6, 3.0);
        }
        Icon::Rss => {
            // the feed sign: a dot and two arcs on an orange tile
            c.fill(Paint::Solid(0xa84c00), |x, y| rounded(x, y, 2.5, 21.5, 3.5));
            c.fill(Paint::Gradient(0xffb347, 0xe8650c), |x, y| rounded(x, y, 3.3, 20.7, 2.8));
            let white = Paint::Solid(0xffffff);
            c.circle(white, 7.8, 16.2, 2.0);
            for (r_out, r_in) in [(9.2, 6.8), (14.2, 11.8)] {
                c.fill(white, move |x, y| {
                    let (dx, dy) = (x - 6.2, y - 17.8);
                    let d = (dx * dx + dy * dy).sqrt();
                    dx >= -0.5 && dy <= 0.5 && d <= r_out && d >= r_in
                });
            }
        }
    }
}

/// Inside a square from `a` to `b` (both axes) with corners rounded by `r`.
fn rounded(x: f32, y: f32, a: f32, b: f32, r: f32) -> bool {
    let cx = x.clamp(a + r, b - r);
    let cy = y.clamp(a + r, b - r);
    x >= a && x <= b && y >= a && y <= b && (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// The application icon: a green disc with a white down arrow over a bar.
fn draw_app(c: &mut Canvas) {
    c.circle(Paint::Solid(0x15651d), 12.0, 12.0, 11.5);
    c.circle(Paint::Gradient(0x6ad86a, 0x1f8f2a), 12.0, 12.0, 10.5);
    let white = Paint::Solid(0xffffff);
    c.poly(
        white,
        &[(9.5, 4.5), (14.5, 4.5), (14.5, 11.0), (18.0, 11.0), (12.0, 17.0), (6.0, 11.0), (9.5, 11.0)],
    );
    c.rect(white, 6.5, 18.0, 17.5, 20.0);
}

/// A toolbar image, `size` pixels square: BGRA bytes, premultiplied, top-down; `gray` for the
/// disabled look.
pub fn toolbar_bgra(icon: Icon, size: usize, gray: bool) -> Vec<u8> {
    let mut c = Canvas::new(size);
    draw(icon, &mut c);
    c.bgra(gray)
}

/// The application icon, `size` pixels square: BGRA bytes, premultiplied, top-down (a Windows
/// DIB).
pub fn app_bgra(size: usize) -> Vec<u8> {
    let mut c = Canvas::new(size);
    draw_app(&mut c);
    c.bgra(false)
}

/// The application icon, `size` pixels square: ARGB bytes in that order, not premultiplied,
/// top-down (a StatusNotifierItem pixmap: 32-bit ARGB in network byte order).
pub fn app_argb(size: usize) -> Vec<u8> {
    let mut c = Canvas::new(size);
    draw_app(&mut c);
    let mut out = Vec::with_capacity(c.px.len() * 4);
    for p in &c.px {
        let a = p[3];
        let un = |v: f32| if a > 0.0 { v / a } else { 0.0 };
        out.extend([to8(a), to8(un(p[0])), to8(un(p[1])), to8(un(p[2]))]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_have_ink_and_transparent_corners() {
        for icon in Icon::ALL {
            let mut c = Canvas::new(24);
            draw(icon, &mut c);
            let ink = c.px.iter().filter(|p| p[3] > 0.5).count();
            assert!(ink > 40, "{icon:?} is nearly empty");
            assert!(c.px[0][3] < 0.01, "{icon:?} corner not transparent");
            assert!(c.px.iter().all(|p| p[0] <= p[3] + 1e-4), "{icon:?} not premultiplied");
            assert_eq!(toolbar_bgra(icon, 16, true).len(), 16 * 16 * 4);
        }
    }

    #[test]
    fn the_app_icon_in_both_layouts() {
        let (bgra, argb) = (app_bgra(22), app_argb(22));
        assert_eq!((bgra.len(), argb.len()), (22 * 22 * 4, 22 * 22 * 4));
        // a corner is clear, the centre opaque; the same pixel in both layouts
        assert_eq!((bgra[3], argb[0]), (0, 0));
        let mid = (11 * 22 + 11) * 4;
        assert_eq!(argb[mid], 255);
        assert_eq!([bgra[mid + 2], bgra[mid + 1], bgra[mid]], [argb[mid + 1], argb[mid + 2], argb[mid + 3]]);
    }
}
