//! The GUIs' icons, shared by the Win32 and GTK clients.
//!
//! - The toolbar images ([`Icon`]): original drawings, a few vector shapes on a 24-unit grid
//!   rasterised at run time with 4×4 supersampling, so they are sharp at every scale
//!   ([`toolbar_bgra`]).
//! - The PatinaTorrent state icons ([`StateIcon`]): the PNG exports of
//!   `assets/patina-icons/states` at 16, 20, 24 and 32 px, embedded. Which one a torrent,
//!   tracker or the network indicator shows is [`crate::status::status_icon`],
//!   [`tracker_icon`] and [`network_icon`], as `states/manifest.json` maps them; this table
//!   (state to asset name) is the same for both GUIs.
//! - The PatinaTorrent application icon's PNG exports ([`app_png`], for the GTK client's window,
//!   tray and desktop entries). The Win32 client embeds the same family's `.ico` as the
//!   executable's icon resource (`crates/win32/build.rs`). The family in use is named once, in
//!   `assets/patina-icons/app-icon.txt` (see `assets/patina-icons/INTEGRATION.md`).
//!
//! The PNGs are decoded by the toolkits (Windows' icon loader, GDK's texture loader), so there
//! is no image decoder here.

use patorrent_core::TrackerStatus;

/// The pixel sizes the state icons are embedded at (`exports/states/<size>`).
pub const STATE_SIZES: [u32; 4] = [16, 20, 24, 32];

/// The state icon size to use for `want` pixels (16 scaled to the screen): the largest
/// embedded size that fits, 16 at least. 100 %, 125 %, 150 % and 200 % get theirs exactly.
pub fn state_size(want: u32) -> u32 {
    STATE_SIZES.iter().rev().copied().find(|&s| s <= want).unwrap_or(STATE_SIZES[0])
}

macro_rules! state_png {
    ($size:literal, $file:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/patina-icons/exports/states/",
            $size,
            "/",
            $file,
            ".png"
        ))
    };
}

macro_rules! state_icons {
    ($($(#[$doc:meta])* $variant:ident => $file:literal,)*) => {
        /// A PatinaTorrent state icon, named after its asset (`states/<name>.svg`, exported as
        /// `exports/states/<size>/<name>.png`). [`StateIcon::ALL`] is in declaration order, so
        /// `icon as usize` is an index into it (the Win32 image list's order).
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum StateIcon {
            $($(#[$doc])* $variant,)*
        }

        impl StateIcon {
            pub const ALL: [StateIcon; 24] = [$(StateIcon::$variant,)*];

            /// The asset's name, as in `states/manifest.json`'s `id`.
            pub fn name(self) -> &'static str {
                match self {
                    $(StateIcon::$variant => $file,)*
                }
            }

            /// The PNG export at [`state_size`]`(size)` pixels: 8-bit RGBA, straight alpha.
            pub fn png(self, size: u32) -> &'static [u8] {
                let i = STATE_SIZES.iter().position(|&s| s == state_size(size)).unwrap_or(0);
                let all: [&'static [u8]; 4] = match self {
                    $(StateIcon::$variant => [
                        state_png!(16, $file),
                        state_png!(20, $file),
                        state_png!(24, $file),
                        state_png!(32, $file),
                    ],)*
                };
                all[i]
            }
        }
    };
}

state_icons! {
    // torrents: the branches of `status::status_text`
    Downloading => "downloading",
    ForceDownloading => "force-downloading",
    Seeding => "seeding",
    ForceSeeding => "force-seeding",
    DownloadingMetadata => "downloading-metadata",
    ForceDownloadingMetadata => "force-downloading-metadata",
    CheckingFiles => "checking-files",
    CheckingResumeData => "checking-resume-data",
    QueuedChecking => "queued-checking",
    QueuedResumeChecking => "queued-resume-checking",
    QueuedDownload => "queued-download",
    QueuedSeed => "queued-seed",
    PausedIncomplete => "paused-incomplete",
    PausedComplete => "paused-complete",
    StoppedIncomplete => "stopped-incomplete",
    /// "Finished": stopped and complete.
    StoppedComplete => "stopped-complete",
    Moving => "moving",
    Error => "error",
    // trackers (the Trackers tab)
    TrackerNotContacted => "tracker-not-contacted",
    TrackerWorking => "tracker-working",
    TrackerUpdating => "tracker-updating",
    TrackerError => "tracker-error",
    // the status bar's network indicator
    /// An incoming connection has been seen.
    NetworkOk => "network-ok",
    /// None seen yet (not proof that the port is closed).
    NetworkNoIncoming => "network-no-incoming",
}

/// A tracker row's icon.
pub fn tracker_icon(status: TrackerStatus) -> StateIcon {
    match status {
        TrackerStatus::NotContacted => StateIcon::TrackerNotContacted,
        TrackerStatus::Working => StateIcon::TrackerWorking,
        TrackerStatus::Updating => StateIcon::TrackerUpdating,
        TrackerStatus::Error => StateIcon::TrackerError,
    }
}

/// The status bar's network indicator (`SessionStats::has_incoming_connections`).
pub fn network_icon(has_incoming_connections: bool) -> StateIcon {
    if has_incoming_connections { StateIcon::NetworkOk } else { StateIcon::NetworkNoIncoming }
}

/// The pixel sizes the application icon is exported at (`exports/app/<family>/<size>.png`; the
/// 512 px master is left out).
pub const APP_SIZES: [u32; 8] = [16, 20, 24, 32, 48, 64, 128, 256];

macro_rules! app_png {
    ($size:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/patina-icons/exports/app/",
            env!("PATINA_APP_ICON"),
            "/",
            $size,
            ".png"
        ))
    };
}

/// The application icon (the family named in `assets/patina-icons/app-icon.txt`) as a PNG of
/// `size` pixels, one of [`APP_SIZES`]: 8-bit RGBA, straight alpha.
pub fn app_png(size: u32) -> Option<&'static [u8]> {
    Some(match size {
        16 => app_png!(16),
        20 => app_png!(20),
        24 => app_png!(24),
        32 => app_png!(32),
        48 => app_png!(48),
        64 => app_png!(64),
        128 => app_png!(128),
        256 => app_png!(256),
        _ => return None,
    })
}

/// The application icon's family, such as `01-verdigris` (`assets/patina-icons/app-icon.txt`).
pub const APP_ICON_FAMILY: &str = env!("PATINA_APP_ICON");

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

/// A toolbar image, `size` pixels square: BGRA bytes, premultiplied, top-down; `gray` for the
/// disabled look.
pub fn toolbar_bgra(icon: Icon, size: usize, gray: bool) -> Vec<u8> {
    let mut c = Canvas::new(size);
    draw(icon, &mut c);
    c.bgra(gray)
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

    /// Width, height, bit depth, colour type and interlace method from a PNG's IHDR.
    fn ihdr(png: &[u8]) -> (u32, u32, u8, u8, u8) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        assert_eq!(&png[12..16], b"IHDR");
        let be = |i: usize| u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]);
        (be(16), be(20), png[24], png[25], png[28])
    }

    #[test]
    fn state_icons_are_embedded_at_every_size() {
        for icon in StateIcon::ALL {
            assert_eq!(StateIcon::ALL[icon as usize], icon);
            for size in STATE_SIZES {
                // 8-bit RGBA, not interlaced, at the size asked for
                assert_eq!(ihdr(icon.png(size)), (size, size, 8, 6, 0), "{icon:?} at {size}");
            }
        }
    }

    #[test]
    fn state_icons_are_the_manifests() {
        let manifest = include_str!("../../../assets/patina-icons/states/manifest.json");
        for icon in StateIcon::ALL {
            let id = format!("\"id\": \"{}\"", icon.name());
            assert!(manifest.contains(&id), "{icon:?}: {id} is not in states/manifest.json");
        }
        let mut names: Vec<&str> = StateIcon::ALL.iter().map(|i| i.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), StateIcon::ALL.len());
    }

    #[test]
    fn state_sizes_follow_the_scale() {
        let got: Vec<u32> = [8, 16, 18, 20, 22, 24, 28, 32, 48].into_iter().map(state_size).collect();
        assert_eq!(got, [16, 16, 16, 20, 20, 24, 24, 32, 32]);
        assert_eq!(ihdr(StateIcon::Error.png(28)).0, 24);
    }

    #[test]
    fn trackers_and_network() {
        assert_eq!(tracker_icon(TrackerStatus::NotContacted), StateIcon::TrackerNotContacted);
        assert_eq!(tracker_icon(TrackerStatus::Working), StateIcon::TrackerWorking);
        assert_eq!(tracker_icon(TrackerStatus::Updating), StateIcon::TrackerUpdating);
        assert_eq!(tracker_icon(TrackerStatus::Error), StateIcon::TrackerError);
        assert_eq!(network_icon(true), StateIcon::NetworkOk);
        assert_eq!(network_icon(false), StateIcon::NetworkNoIncoming);
    }

    #[test]
    fn the_app_icon_at_every_size() {
        let chosen = include_str!("../../../assets/patina-icons/app-icon.txt").trim();
        assert_eq!(APP_ICON_FAMILY, chosen);
        for size in APP_SIZES {
            let png = app_png(size).expect("an exported size");
            assert_eq!(ihdr(png), (size, size, 8, 6, 0), "app icon at {size}");
        }
        assert!(app_png(22).is_none() && app_png(512).is_none());
    }
}
