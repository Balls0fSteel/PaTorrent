//! The detail tabs under the torrent list: General, Trackers, Peers, Pieces, Files, Speed and
//! Logger. List tabs are owner-data list views; General, Pieces and Speed are painted panels.
//! The Pieces panel paints the piece map and holds a list view of the pieces being downloaded
//! under it (µTorrent 2.2.1's Pieces list), its Blocks column drawn by [`block_bar`].

use patorrent_core::{PartialPiece, TorrentStatus};
use patorrent_frontend::detail::{self, BlockKind, PieceShade, SPEED_SAMPLES};
pub use patorrent_frontend::detail::{Detail, Tab as Page};
use patorrent_frontend::{columns::RowExtra, fmt, tr};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Foundation::{COLORREF, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_BTNSHADOW, COLOR_GRAYTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreatePen,
    CreateSolidBrush, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_RTLREADING, DT_SINGLELINE,
    DT_VCENTER, DeleteObject, DrawTextW, FillRect, FrameRect, GetSysColor, GetSysColorBrush, HDC, HFONT,
    LineTo, MoveToEx, PS_SOLID, Polyline, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};

use crate::util::{rtl, scale, wide};

fn rgb(c: u32) -> COLORREF {
    ((c & 0xff) << 16) | (c & 0xff00) | ((c >> 16) & 0xff)
}

const BAR_BLUE: u32 = 0x3c78d8;
const BAR_GREEN: u32 = 0x4bb543;
const BLOCK_REQUESTED: u32 = 0xe8a000;
const DOWN_COLOR: u32 = 0x2aa52a;
const UP_COLOR: u32 = 0xd23c3c;

unsafe fn fill(hdc: HDC, rc: &RECT, color: u32) {
    unsafe {
        let b = CreateSolidBrush(rgb(color));
        FillRect(hdc, rc, b);
        DeleteObject(b);
    }
}

unsafe fn text(hdc: HDC, s: &str, rc: &RECT, flags: u32) {
    let w = wide(s);
    let mut r = *rc;
    let flags = flags | DT_SINGLELINE | DT_NOPREFIX | DT_VCENTER | rtl(DT_RTLREADING);
    unsafe { DrawTextW(hdc, w.as_ptr(), -1, &mut r, flags) };
}

/// A horizontal bar with one cell per piece: `value(i)` in 0..=1 is the fill strength.
unsafe fn piece_bar(hdc: HDC, rc: &RECT, pieces: usize, color: u32, value: impl Fn(usize) -> f32) {
    unsafe {
        FrameRect(hdc, rc, GetSysColorBrush(COLOR_BTNSHADOW));
        let inner = RECT { left: rc.left + 1, top: rc.top + 1, right: rc.right - 1, bottom: rc.bottom - 1 };
        let w = (inner.right - inner.left).max(0) as usize;
        fill(hdc, &inner, 0xffffff);
        if pieces == 0 || w == 0 {
            return;
        }
        // One column of pixels per step, averaging the pieces it covers.
        let mut x = 0;
        while x < w {
            let (a, b) = (x * pieces / w, ((x + 1) * pieces / w).max(x * pieces / w + 1).min(pieces));
            let v = (a..b).map(&value).sum::<f32>() / (b - a) as f32;
            // Extend the run while the value stays the same, to draw fewer rectangles.
            let mut end = x + 1;
            while end < w {
                let (c, d) =
                    (end * pieces / w, ((end + 1) * pieces / w).max(end * pieces / w + 1).min(pieces));
                let u = (c..d).map(&value).sum::<f32>() / (d - c) as f32;
                if (u - v).abs() > 0.02 {
                    break;
                }
                end += 1;
            }
            if v > 0.0 {
                let mix = |c: u32, shift: u32| {
                    let ch = ((c >> shift) & 0xff) as f32;
                    ((255.0 - (255.0 - ch) * v.min(1.0)) as u32) << shift
                };
                let c = mix(color, 16) | mix(color, 8) | mix(color, 0);
                let r = RECT {
                    left: inner.left + x as i32,
                    top: inner.top,
                    right: inner.left + end as i32,
                    bottom: inner.bottom,
                };
                fill(hdc, &r, c);
            }
            x = end;
        }
    }
}

pub struct PaintCtx<'a> {
    pub hdc: HDC,
    pub rc: RECT,
    pub dpi: u32,
    pub font: HFONT,
    pub bold: HFONT,
    pub status: Option<(&'a TorrentStatus, RowExtra<'a>)>,
    pub detail: &'a Detail,
}

impl PaintCtx<'_> {
    fn s(&self, v: i32) -> i32 {
        scale(v, self.dpi)
    }
}

pub unsafe fn paint(page: Page, c: &PaintCtx<'_>) {
    unsafe {
        let face = if page == Page::Speed { COLOR_WINDOW } else { COLOR_BTNFACE };
        FillRect(c.hdc, &c.rc, GetSysColorBrush(face));
        SetBkMode(c.hdc, TRANSPARENT as i32);
        SetTextColor(c.hdc, GetSysColor(COLOR_WINDOWTEXT));
        let old = SelectObject(c.hdc, c.font);
        match page {
            Page::General => general(c),
            Page::Pieces => pieces(c),
            Page::Speed => speed(c),
            _ => {}
        }
        SelectObject(c.hdc, old);
    }
}

unsafe fn general(c: &PaintCtx<'_>) {
    let Some((st, x)) = c.status else {
        unsafe { hint(c, tr!(GenSelectTorrent)) };
        return;
    };
    let g = detail::general(st, x.user_paused);
    let pad = c.s(8);
    let line = c.s(18);
    let label_w = c.s(110);
    let left = c.rc.left + pad;
    let right = c.rc.right - pad;
    let mut y = c.rc.top + pad;
    unsafe {
        // Downloaded and availability bars.
        let pieces = c.detail.have.as_ref().map_or(st.pieces_total as usize, |h| h.len());
        for (i, (label, value)) in g.bars.iter().enumerate() {
            let row = RECT { left, top: y, right, bottom: y + line };
            text(c.hdc, label, &row, DT_LEFT);
            let value_w = c.s(60);
            let bar = RECT {
                left: left + label_w,
                top: y + c.s(3),
                right: (right - value_w - pad).max(left + label_w),
                bottom: y + line - c.s(3),
            };
            let detail = c.detail;
            if i == 0 {
                match &detail.have {
                    Some(h) => piece_bar(c.hdc, &bar, pieces, BAR_BLUE, |p| if h.get(p) { 1.0 } else { 0.0 }),
                    None => {
                        let done = st.progress_ppm as usize;
                        piece_bar(c.hdc, &bar, 1000, BAR_BLUE, |p| if p * 1000 < done { 1.0 } else { 0.0 })
                    }
                }
            } else {
                let av = &detail.availability;
                let have = detail.have.as_ref();
                piece_bar(c.hdc, &bar, av.len(), BAR_GREEN, |p| {
                    if have.is_some_and(|h| h.get(p)) { 1.0 } else { (av[p] as f32 / 3.0).min(1.0) }
                });
            }
            let vr = RECT { left: right - value_w, top: y, right, bottom: y + line };
            text(c.hdc, value, &vr, DT_RIGHT);
            y += line + c.s(2);
        }

        y += c.s(6);
        y = section(c, tr!(GenTransfer), y);
        let col_w = (right - left) / 3;
        for row in &g.transfer {
            for (i, (label, value)) in row.iter().enumerate() {
                let x0 = left + col_w * i as i32;
                pair(c, x0, y, col_w, label, value);
            }
            y += line;
        }
        pair(c, left, y, right - left, g.status.0, &g.status.1);
        y += line + c.s(6);

        y = section(c, tr!(GenGeneral), y);
        let half = (right - left) * 2 / 3;
        for [(l1, v1), (l2, v2)] in &g.general {
            pair(c, left, y, half, l1, v1);
            pair(c, left + half, y, right - left - half, l2, v2);
            y += line;
        }
        pair(c, left, y, right - left, g.hash.0, &g.hash.1);
    }
}

unsafe fn section(c: &PaintCtx<'_>, title: &str, y: i32) -> i32 {
    let line = c.s(18);
    let left = c.rc.left + c.s(8);
    let right = c.rc.right - c.s(8);
    unsafe {
        let old = SelectObject(c.hdc, c.bold);
        text(c.hdc, title, &RECT { left, top: y, right, bottom: y + line }, DT_LEFT);
        SelectObject(c.hdc, old);
        let pen = CreatePen(PS_SOLID, 1, GetSysColor(COLOR_BTNSHADOW));
        let old = SelectObject(c.hdc, pen);
        let mid = y + line / 2;
        MoveToEx(c.hdc, left + c.s(70), mid, std::ptr::null_mut());
        LineTo(c.hdc, right, mid);
        SelectObject(c.hdc, old);
        DeleteObject(pen);
    }
    y + line
}

unsafe fn pair(c: &PaintCtx<'_>, x: i32, y: i32, w: i32, label: &str, value: &str) {
    let line = c.s(18);
    let label_w = c.s(105).min(w / 2);
    unsafe {
        let old = SelectObject(c.hdc, c.bold);
        text(c.hdc, label, &RECT { left: x, top: y, right: x + label_w, bottom: y + line }, DT_LEFT);
        SelectObject(c.hdc, old);
        let r = RECT { left: x + label_w, top: y, right: x + w - c.s(6), bottom: y + line };
        text(c.hdc, value, &r, DT_LEFT | DT_END_ELLIPSIS);
    }
}

unsafe fn hint(c: &PaintCtx<'_>, s: &str) {
    unsafe {
        SetTextColor(c.hdc, GetSysColor(COLOR_GRAYTEXT));
        let r = RECT {
            left: c.rc.left + c.s(8),
            top: c.rc.top + c.s(8),
            right: c.rc.right,
            bottom: c.rc.top + c.s(26),
        };
        text(c.hdc, s, &r, DT_LEFT);
    }
}

/// Metrics of the piece map: (padding, square, gap, header line).
fn map_metrics(dpi: u32) -> (i32, i32, i32, i32) {
    (scale(8, dpi), scale(9, dpi), scale(1, dpi).max(1), scale(22, dpi))
}

/// Height of the piece map at the top of the Pieces page `w` by `h` pixels: the header line
/// plus the rows of squares the pieces need, at most two fifths of the page (the pieces list
/// takes the rest).
pub fn piece_map_height(d: &Detail, w: i32, h: i32, dpi: u32) -> i32 {
    let (pad, cell, gap, header) = map_metrics(dpi);
    let n = d.have.as_ref().map_or(0, |b| b.len());
    if n == 0 {
        return scale(26, dpi).min(h.max(0));
    }
    let cols = ((w - 2 * pad) / (cell + gap)).max(1) as usize;
    let rows = n.div_ceil(cols) as i32;
    let need = header + rows * (cell + gap) + pad;
    let cap = (h * 2 / 5).max(header + cell + gap + pad);
    need.min(cap).min(h.max(0))
}

/// The Blocks cell of a Pieces list row: the blocks left to right, blue on disk, green being
/// written, orange requested, white free, in a grey frame.
pub unsafe fn block_bar(hdc: HDC, rc: &RECT, p: &PartialPiece) {
    let n = p.blocks.len() as i64;
    let w = (rc.right - rc.left - 2) as i64;
    if n == 0 || w <= 0 {
        return;
    }
    unsafe {
        fill(hdc, rc, 0xffffff);
        for (kind, start, len) in detail::block_runs(p) {
            let color = match kind {
                BlockKind::Free => continue,
                BlockKind::Requested => BLOCK_REQUESTED,
                BlockKind::Writing => BAR_GREEN,
                BlockKind::Finished => BAR_BLUE,
            };
            let x0 = rc.left + 1 + (start as i64 * w / n) as i32;
            // at least a pixel, so one requested block among a thousand still shows
            let x1 = (rc.left + 1 + ((start + len) as i64 * w / n) as i32).max(x0 + 1);
            fill(hdc, &RECT { left: x0, top: rc.top + 1, right: x1, bottom: rc.bottom - 1 }, color);
        }
        FrameRect(hdc, rc, GetSysColorBrush(COLOR_BTNSHADOW));
    }
}

unsafe fn pieces(c: &PaintCtx<'_>) {
    let d = c.detail;
    let Some(have) = &d.have else {
        let msg = if c.status.is_some() { tr!(PiecesWaiting) } else { tr!(PiecesSelectTorrent) };
        unsafe { hint(c, msg) };
        return;
    };
    let n = have.len();
    if n == 0 {
        return;
    }
    let (pad, cell, gap, header) = map_metrics(c.dpi);
    let cols = ((c.rc.right - c.rc.left - 2 * pad) / (cell + gap)).max(1) as usize;
    // the pieces list covers the rest of the page
    let bottom = c.rc.top + piece_map_height(d, c.rc.right - c.rc.left, c.rc.bottom - c.rc.top, c.dpi);
    unsafe {
        let r = RECT {
            left: c.rc.left + pad,
            top: c.rc.top + c.s(4),
            right: c.rc.right - pad,
            bottom: c.rc.top + header,
        };
        text(c.hdc, &d.pieces_summary(), &r, DT_LEFT | DT_END_ELLIPSIS);
        let rows_in = |h: i32| (h / (cell + gap)).max(0) as usize;
        let mut max_rows = rows_in(bottom - c.rc.top - header - pad);
        if n > cols * max_rows {
            // leave a line for "... more"
            max_rows = rows_in(bottom - c.rc.top - header - pad - c.s(18));
        }
        let shown = n.min(cols * max_rows);
        for i in 0..shown {
            let x = c.rc.left + pad + ((i % cols) as i32) * (cell + gap);
            let y = c.rc.top + header + ((i / cols) as i32) * (cell + gap);
            let rc = RECT { left: x, top: y, right: x + cell, bottom: y + cell };
            let color = match d.piece_shade(i) {
                PieceShade::Have => BAR_BLUE,
                PieceShade::Downloading => BLOCK_REQUESTED,
                PieceShade::Available(0) => 0xffffff,
                PieceShade::Available(1) => 0xd8e8d0,
                PieceShade::Available(2 | 3) => 0xa8d29a,
                PieceShade::Available(_) => 0x6fb85c,
            };
            fill(c.hdc, &rc, color);
            FrameRect(c.hdc, &rc, GetSysColorBrush(COLOR_BTNSHADOW));
        }
        if shown < n {
            let y = bottom - c.s(18);
            let r = RECT { left: c.rc.left + pad, top: y, right: c.rc.right - pad, bottom };
            text(c.hdc, &tr!(PiecesMore, count = n - shown), &r, DT_RIGHT);
        }
    }
}

unsafe fn speed(c: &PaintCtx<'_>) {
    let d = c.detail;
    let pad = c.s(8);
    let legend = c.s(20);
    let plot = RECT {
        left: c.rc.left + pad + c.s(70),
        top: c.rc.top + pad + legend,
        right: c.rc.right - pad,
        bottom: c.rc.bottom - pad,
    };
    if plot.right - plot.left < 20 || plot.bottom - plot.top < 20 {
        return;
    }
    let top = detail::speed_scale(&d.speed);
    unsafe {
        FrameRect(c.hdc, &plot, GetSysColorBrush(COLOR_BTNSHADOW));
        let grid = CreatePen(PS_SOLID, 1, rgb(0xe0e0e0));
        let old = SelectObject(c.hdc, grid);
        for i in 1..4 {
            let y = plot.top + (plot.bottom - plot.top) * i / 4;
            MoveToEx(c.hdc, plot.left + 1, y, std::ptr::null_mut());
            LineTo(c.hdc, plot.right - 1, y);
            let v = top * (4 - i) as u64 / 4;
            let r = RECT {
                left: c.rc.left + pad,
                top: y - c.s(8),
                right: plot.left - c.s(4),
                bottom: y + c.s(8),
            };
            text(c.hdc, &fmt::speed_always(v as u32), &r, DT_RIGHT);
        }
        let r = RECT {
            left: c.rc.left + pad,
            top: plot.top - c.s(8),
            right: plot.left - c.s(4),
            bottom: plot.top + c.s(8),
        };
        text(c.hdc, &fmt::speed_always(top as u32), &r, DT_RIGHT);
        SelectObject(c.hdc, old);
        DeleteObject(grid);

        let (down, up) = d.speed.back().copied().unwrap_or((0, 0));
        let mut x = plot.left;
        for (label, color) in [
            (tr!(SpeedDownload, rate = fmt::speed_always(down)), DOWN_COLOR),
            (tr!(SpeedUpload, rate = fmt::speed_always(up)), UP_COLOR),
        ] {
            let swatch = RECT {
                left: x,
                top: c.rc.top + pad + c.s(4),
                right: x + c.s(10),
                bottom: c.rc.top + pad + c.s(14),
            };
            fill(c.hdc, &swatch, color);
            let r = RECT {
                left: x + c.s(14),
                top: c.rc.top + pad,
                right: x + c.s(200),
                bottom: c.rc.top + pad + c.s(18),
            };
            text(c.hdc, &label, &r, DT_LEFT);
            x += c.s(200);
        }

        let w = (plot.right - plot.left - 2) as usize;
        let h = (plot.bottom - plot.top - 2) as f64;
        for (series, color) in [(0usize, DOWN_COLOR), (1, UP_COLOR)] {
            let n = d.speed.len();
            if n < 2 {
                continue;
            }
            let pts: Vec<POINT> = d
                .speed
                .iter()
                .enumerate()
                .map(|(i, &(a, b))| {
                    let v = if series == 0 { a } else { b } as f64;
                    // Newest sample at the right edge; one sample per pixel column at most.
                    let step = (w as f64 / (SPEED_SAMPLES - 1) as f64).max(1.0);
                    let x = plot.right - 1 - ((n - 1 - i) as f64 * step) as i32;
                    let y = plot.bottom - 1 - (v / top as f64 * h) as i32;
                    POINT { x: x.max(plot.left + 1), y }
                })
                .collect();
            let pen = CreatePen(PS_SOLID, scale(2, c.dpi).max(1), rgb(color));
            let old = SelectObject(c.hdc, pen);
            Polyline(c.hdc, pts.as_ptr(), pts.len() as i32);
            SelectObject(c.hdc, old);
            DeleteObject(pen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patorrent_core::Bitfield;

    #[test]
    fn the_piece_map_leaves_the_rest_of_the_page_to_the_list() {
        let mut d = Detail::default();
        // before the first reply: one hint line
        assert_eq!(piece_map_height(&d, 500, 300, 96), 26);
        // 10 pieces fit on one row of 49 squares: header, a row, padding
        d.have = Some(Bitfield::new(10, false));
        assert_eq!(piece_map_height(&d, 500, 300, 96), 22 + 10 + 8);
        // 5,000 pieces would need 103 rows: two fifths of the page
        d.have = Some(Bitfield::new(5000, false));
        assert_eq!(piece_map_height(&d, 500, 300, 96), 120);
        // a very short page still shows the header and a row, never more than the page
        assert_eq!(piece_map_height(&d, 500, 50, 96), 40);
        assert_eq!(piece_map_height(&d, 500, 30, 96), 30);
        // twice the size at 192 DPI
        d.have = Some(Bitfield::new(10, false));
        assert_eq!(piece_map_height(&d, 1000, 600, 192), 80);
    }
}
