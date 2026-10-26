//! The GUIs' icons, shared by the Win32 and GTK clients.
//!
//! - The state icons ([`StateIcon`]) and the toolbar images ([`Icon`]): the bold UI artwork's
//!   PNG exports (`assets/ui-icon-explorations/png{,-dark}/bold/<size>/<id>.png`, decisions
//!   D27-D30), embedded. Which state icon a torrent, tracker or the network indicator shows is
//!   [`crate::status::status_icon`], [`tracker_icon`] and [`network_icon`]; this table (state
//!   to asset name) is the same for both GUIs. Each comes in the light palette and the dark one
//!   (for a dark GTK theme). The Win32 toolbar wants pixels, so [`toolbar_bgra`] decodes its
//!   PNG ([`decode_png`]).
//! - The PatinaTorrent application icon's PNG exports ([`app_png`], for the GTK client's window,
//!   tray and desktop entries). The Win32 client embeds the same family's `.ico` as the
//!   executable's icon resource (`crates/win32/build.rs`). The family in use is named once, in
//!   `assets/patina-icons/app-icon.txt` (see `assets/patina-icons/INTEGRATION.md`).
//!
//! Otherwise the PNGs are decoded by the toolkits (Windows' icon loader, GDK's texture loader).

use patorrent_core::TrackerStatus;

/// The pixel sizes the state icons are embedded at.
pub const STATE_SIZES: [u32; 4] = [16, 20, 24, 32];

/// The state icon size to use for `want` pixels (16 scaled to the screen): the largest
/// embedded size that fits, 16 at least. 100 %, 125 %, 150 % and 200 % get theirs exactly.
pub fn state_size(want: u32) -> u32 {
    STATE_SIZES.iter().rev().copied().find(|&s| s <= want).unwrap_or(STATE_SIZES[0])
}

/// One export of the bold UI artwork: `$dir` is `png` (light palette) or `png-dark`.
macro_rules! bold_png {
    ($dir:literal, $size:literal, $file:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/ui-icon-explorations/",
            $dir,
            "/bold/",
            $size,
            "/",
            $file,
            ".png"
        ))
    };
}

macro_rules! state_icons {
    ($($(#[$doc:meta])* $variant:ident => $file:literal,)*) => {
        /// A state icon, named after its artwork ID (`manifest.json`'s `id` and `runtime` list in
        /// `assets/ui-icon-explorations`). [`StateIcon::ALL`] is in declaration order, so
        /// `icon as usize` is an index into it (the Win32 image list's order).
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum StateIcon {
            $($(#[$doc])* $variant,)*
        }

        impl StateIcon {
            pub const ALL: [StateIcon; 24] = [$(StateIcon::$variant,)*];

            /// The artwork ID.
            pub fn name(self) -> &'static str {
                match self {
                    $(StateIcon::$variant => $file,)*
                }
            }

            /// The light-palette PNG at [`state_size`]`(size)` pixels: 8-bit RGBA, straight alpha.
            pub fn png(self, size: u32) -> &'static [u8] {
                self.themed_png(size, false)
            }

            /// The PNG at [`state_size`]`(size)` pixels, in the dark palette with `dark`.
            pub fn themed_png(self, size: u32, dark: bool) -> &'static [u8] {
                let i = STATE_SIZES.iter().position(|&s| s == state_size(size)).unwrap_or(0);
                let all: [[&'static [u8]; 4]; 2] = match self {
                    $(StateIcon::$variant => [
                        [
                            bold_png!("png", 16, $file),
                            bold_png!("png", 20, $file),
                            bold_png!("png", 24, $file),
                            bold_png!("png", 32, $file),
                        ],
                        [
                            bold_png!("png-dark", 16, $file),
                            bold_png!("png-dark", 20, $file),
                            bold_png!("png-dark", 24, $file),
                            bold_png!("png-dark", 32, $file),
                        ],
                    ],)*
                };
                all[dark as usize][i]
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

/// The pixel sizes the toolbar images are embedded at.
pub const TOOLBAR_SIZES: [u32; 5] = [16, 20, 24, 32, 48];

/// The toolbar image size to use for `want` pixels: the largest embedded size that fits, 16 at
/// least. The Win32 toolbar asks for 24 scaled to the screen: 24, 24, 32 and 48 px at 100 %,
/// 125 %, 150 % and 200 %.
pub fn toolbar_size(want: u32) -> u32 {
    TOOLBAR_SIZES.iter().rev().copied().find(|&s| s <= want).unwrap_or(TOOLBAR_SIZES[0])
}

macro_rules! toolbar_icons {
    ($($variant:ident => $file:literal,)*) => {
        /// Toolbar images, in image-list order, named after their artwork IDs (`manifest.json`'s
        /// `toolbar` list in `assets/ui-icon-explorations`).
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum Icon {
            $($variant,)*
        }

        impl Icon {
            pub const ALL: [Icon; 11] = [$(Icon::$variant,)*];

            /// The artwork ID.
            pub fn name(self) -> &'static str {
                match self {
                    $(Icon::$variant => $file,)*
                }
            }

            /// The PNG at [`toolbar_size`]`(size)` pixels, in the dark palette with `dark`: 8-bit
            /// RGBA, straight alpha.
            pub fn png(self, size: u32, dark: bool) -> &'static [u8] {
                let i = TOOLBAR_SIZES.iter().position(|&s| s == toolbar_size(size)).unwrap_or(0);
                let all: [[&'static [u8]; 5]; 2] = match self {
                    $(Icon::$variant => [
                        [
                            bold_png!("png", 16, $file),
                            bold_png!("png", 20, $file),
                            bold_png!("png", 24, $file),
                            bold_png!("png", 32, $file),
                            bold_png!("png", 48, $file),
                        ],
                        [
                            bold_png!("png-dark", 16, $file),
                            bold_png!("png-dark", 20, $file),
                            bold_png!("png-dark", 24, $file),
                            bold_png!("png-dark", 32, $file),
                            bold_png!("png-dark", 48, $file),
                        ],
                    ],)*
                };
                all[dark as usize][i]
            }
        }
    };
}

toolbar_icons! {
    AddTorrent => "add-torrent",
    AddUrl => "add-url",
    Create => "create-torrent",
    Remove => "remove",
    Start => "start",
    Pause => "pause",
    Stop => "stop",
    Up => "queue-up",
    Down => "queue-down",
    Preferences => "preferences",
    Rss => "rss",
}

/// A toolbar image, `size` pixels square: BGRA bytes, premultiplied, top-down; `gray` for the
/// disabled look. The light-palette image at [`toolbar_size`]`(size)`, centred when that is
/// smaller than `size`.
pub fn toolbar_bgra(icon: Icon, size: usize, gray: bool) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    let Some((w, h, rgba)) = decode_png(icon.png(size as u32, false)) else { return out };
    let (w, h) = (w as usize, h as usize);
    let (x0, y0) = (size.saturating_sub(w) / 2, size.saturating_sub(h) / 2);
    for y in 0..h.min(size) {
        for x in 0..w.min(size) {
            let p = &rgba[(y * w + x) * 4..][..4];
            let a = p[3] as f32 / 255.0;
            let (mut r, mut g, mut b) =
                (p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a);
            let mut alpha = a;
            if gray {
                // Disabled look: grey at half opacity, premultiplied.
                let l = (0.3 * r + 0.59 * g + 0.11 * b) * 0.6 + 0.35 * a;
                (r, g, b) = (l * 0.5, l * 0.5, l * 0.5);
                alpha = a * 0.5;
            }
            let o = ((y0 + y) * size + x0 + x) * 4;
            out[o..o + 4].copy_from_slice(&[to8(b), to8(g), to8(r), to8(alpha)]);
        }
    }
    out
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Decodes the kind of PNG embedded here (8-bit RGBA, not interlaced, as the artwork pipeline
/// writes them): width, height and straight-alpha RGBA rows. `None` for anything else.
pub fn decode_png(png: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    if png.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let (mut at, mut size, mut idat) = (8, None, Vec::new());
    while at + 12 <= png.len() {
        let len = be(&png[at..]) as usize;
        let kind = &png[at + 4..at + 8];
        let data = png.get(at + 8..at + 8 + len)?;
        match kind {
            // width, height, bit depth 8, colour type 6 (RGBA), no interlace
            b"IHDR" if data.len() == 13 && data[8..] == [8, 6, 0, 0, 0] => {
                size = Some((be(data), be(&data[4..])))
            }
            b"IHDR" => return None,
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    let (w, h) = size?;
    let stride = w as usize * 4;
    let rows = h as usize * (stride + 1);
    // a zlib stream: a two-byte header (deflate, no preset dictionary), then raw deflate
    if idat.len() < 2 || idat[0] & 0x0f != 8 || idat[1] & 0x20 != 0 {
        return None;
    }
    let (raw, _) = patorrent_core::util::puff::inflate(&idat[2..], rows).ok()?;
    if raw.len() != rows {
        return None;
    }
    let mut out = vec![0u8; h as usize * stride];
    for y in 0..h as usize {
        let filter = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..][..stride];
        for x in 0..stride {
            let a = if x >= 4 { out[y * stride + x - 4] } else { 0 };
            let b = if y > 0 { out[(y - 1) * stride + x] } else { 0 };
            let c = if x >= 4 && y > 0 { out[(y - 1) * stride + x - 4] } else { 0 };
            let predict = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => {
                    // Paeth
                    let p = a as i16 + b as i16 - c as i16;
                    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => return None,
            };
            out[y * stride + x] = line[x].wrapping_add(predict);
        }
    }
    Some((w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_images_decode_at_every_size() {
        for icon in Icon::ALL {
            for size in TOOLBAR_SIZES {
                for dark in [false, true] {
                    let (w, h, rgba) = decode_png(icon.png(size, dark)).expect("a decodable PNG");
                    assert_eq!((w, h), (size, size), "{icon:?} at {size}");
                    let ink = rgba.chunks(4).filter(|p| p[3] > 128).count();
                    assert!(ink > 10, "{icon:?} at {size} is nearly empty");
                    assert_eq!(rgba[3], 0, "{icon:?} at {size}: corner not transparent");
                }
            }
            // a slot bigger than the export: the image centred, premultiplied
            let bgra = toolbar_bgra(icon, 22, false);
            assert_eq!(bgra.len(), 22 * 22 * 4);
            assert!(bgra.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "{icon:?}");
            assert!(bgra.chunks(4).any(|p| p[3] > 0), "{icon:?} drew nothing");
            let gray = toolbar_bgra(icon, 24, true);
            assert!(gray.chunks(4).all(|p| p[3] <= 128 && p[0] == p[1] && p[1] == p[2]), "{icon:?}");
        }
    }

    #[test]
    fn toolbar_sizes_follow_the_scale() {
        let got: Vec<u32> = [8, 16, 24, 30, 36, 42, 48, 60].into_iter().map(toolbar_size).collect();
        assert_eq!(got, [16, 16, 24, 24, 32, 32, 48, 48]);
    }

    #[test]
    fn decode_png_rejects_other_kinds() {
        assert!(decode_png(b"not a png").is_none());
        let mut png = StateIcon::Error.png(16).to_vec();
        png[25] = 2; // colour type RGB
        assert!(decode_png(&png).is_none());
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
                for dark in [false, true] {
                    // 8-bit RGBA, not interlaced, at the size asked for
                    let png = icon.themed_png(size, dark);
                    assert_eq!(ihdr(png), (size, size, 8, 6, 0), "{icon:?} at {size}");
                }
                assert_ne!(icon.png(size), icon.themed_png(size, true), "{icon:?}: one palette");
            }
        }
    }

    /// The names in the artwork manifest's `key` list (`"key": [ "a", "b", ... ]`).
    fn manifest_list(key: &str) -> Vec<String> {
        let manifest = include_str!("../../../assets/ui-icon-explorations/manifest.json");
        let start = manifest.find(&format!("\"{key}\": [")).expect(key);
        let list = &manifest[start..];
        let list = &list[list.find('[').unwrap() + 1..list.find(']').unwrap()];
        list.split(',').map(|s| s.trim().trim_matches('"').to_owned()).filter(|s| !s.is_empty()).collect()
    }

    #[test]
    fn icons_are_the_manifests() {
        let state: Vec<&str> = StateIcon::ALL.iter().map(|i| i.name()).collect();
        assert_eq!(state, manifest_list("runtime"));
        let mut toolbar: Vec<&str> = Icon::ALL.iter().map(|i| i.name()).collect();
        let mut listed = manifest_list("toolbar");
        toolbar.sort_unstable();
        listed.sort_unstable();
        assert_eq!(toolbar, listed);
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
