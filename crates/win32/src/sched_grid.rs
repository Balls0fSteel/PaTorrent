//! Preferences > Scheduler's table: 7 days × 24 hours, one colored cell per hour, as in
//! µTorrent 2.2.1. A click moves a cell to the next mode; dragging paints the cells passed over
//! with that same mode. The legend and the hovered hour's text are drawn below the cells.
//!
//! A window class of its own (`PaTorrentSchedGrid`) for the dialog template; the dialog writes
//! and reads the table with [`SET`] and [`GET`].

use std::mem::zeroed;
use std::ptr::null;

use patorrent_frontend::lang;
use patorrent_frontend::scheduler::{CELLS, DAY_NAMES, DAYS, HOURS, Mode, drag_line, hover_text};
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateSolidBrush, DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_RTLREADING,
    DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect, GetSysColor,
    GetSysColorBrush, HDC, HFONT, HGDIOBJ, InvalidateRect, PAINTSTRUCT, SRCCOPY, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{IsWindowEnabled, ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GWLP_USERDATA, GetClientRect, GetParent, GetWindowLongPtrW, IDC_ARROW, LoadCursorW,
    RegisterClassExW, SetWindowLongPtrW, WM_ENABLE, WM_ERASEBKGND, WM_GETFONT, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_USER, WNDCLASSEXW,
};

use crate::util::{point_from_lparam, rtl, send, wide};

pub const CLASS: &str = "PaTorrentSchedGrid";

/// `lParam` points to a `[Mode; CELLS]` to show.
pub const SET: u32 = WM_USER + 1;
/// `lParam` points to a `[Mode; CELLS]` to fill with the table as edited.
pub const GET: u32 = WM_USER + 2;

struct Grid {
    table: [Mode; CELLS],
    /// The mode a drag paints and the cell it last painted, while the button is down.
    painting: Option<(Mode, usize)>,
    hover: Option<usize>,
}

/// `0xRRGGBB` as GDI wants it (BGR).
fn bgr(c: u32) -> COLORREF {
    ((c & 0xff) << 16) | (c & 0xff00) | (c >> 16)
}

/// A cell's colour: the mode's while the scheduler is enabled; greyed, as µTorrent 2.2.1 draws
/// the disabled table (its cells all white, its legend in greys: 0x38 Full speed, 0xa2 Limited,
/// 0xd4 Seeding only).
fn color(m: Mode, enabled: bool) -> COLORREF {
    if enabled {
        return bgr(m.rgb());
    }
    let g = match m {
        Mode::Full => 0x38,
        Mode::Limited => 0xa2,
        Mode::Off => 0xff,
        Mode::SeedOnly => 0xd4,
    };
    g | (g << 8) | (g << 16)
}

pub fn register() {
    let name = wide(CLASS);
    unsafe {
        let mut wc: WNDCLASSEXW = zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(proc_);
        wc.hInstance = GetModuleHandleW(null());
        wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        wc.lpszClassName = name.as_ptr();
        // registering twice (a second Preferences) fails harmlessly
        RegisterClassExW(&wc);
    }
}

/// Where things go in a client area `w` × `h` pixels.
pub(crate) struct Layout {
    label_w: i32,
    top: i32,
    cell_w: i32,
    cell_h: i32,
    legend_y: i32,
    line_h: i32,
}

impl Layout {
    pub(crate) fn new(w: i32, h: i32) -> Layout {
        // label column, hour row, 7 rows, legend line, hover line
        let line_h = (h / 11).max(8);
        let label_w = (w / 9).max(16);
        let cell_w = ((w - label_w) / HOURS as i32).max(2);
        let top = line_h;
        let cell_h = ((h - 3 * line_h - 4) / DAYS as i32).max(4);
        Layout { label_w, top, cell_w, cell_h, legend_y: top + cell_h * DAYS as i32 + 4, line_h }
    }

    pub(crate) fn cell_rect(&self, day: usize, hour: usize) -> RECT {
        let left = self.label_w + hour as i32 * self.cell_w;
        let top = self.top + day as i32 * self.cell_h;
        RECT { left, top, right: left + self.cell_w + 1, bottom: top + self.cell_h + 1 }
    }

    fn hit(&self, x: i32, y: i32) -> Option<usize> {
        if x < self.label_w || y < self.top {
            return None;
        }
        let (hour, day) = ((x - self.label_w) / self.cell_w, (y - self.top) / self.cell_h);
        ((hour as usize) < HOURS && (day as usize) < DAYS).then(|| day as usize * HOURS + hour as usize)
    }
}

unsafe fn layout(hwnd: HWND) -> Layout {
    let mut rc: RECT = unsafe { zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    Layout::new(rc.right, rc.bottom)
}

/// How wide `s` is in `dc`'s font.
unsafe fn text_width(dc: HDC, s: &str) -> i32 {
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { DrawTextW(dc, w.as_ptr(), w.len() as i32, &mut r, DT_CALCRECT | DT_SINGLELINE) };
    r.right - r.left
}

unsafe fn text(dc: HDC, s: &str, rc: &RECT, flags: u32) {
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut r = *rc;
    let flags = flags | DT_SINGLELINE | DT_VCENTER | rtl(DT_RTLREADING);
    unsafe { DrawTextW(dc, w.as_ptr(), w.len() as i32, &mut r, flags) };
}

/// Paints the whole control into a memory bitmap, then copies it to the window in one go: the
/// cells repaint at every mouse move while dragging, and painting them one by one on the
/// screen flickered.
unsafe fn paint(hwnd: HWND, g: &Grid) {
    unsafe {
        let mut ps: PAINTSTRUCT = zeroed();
        let window = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = zeroed();
        GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right.max(1), rc.bottom.max(1));
        let dc = CreateCompatibleDC(window);
        let bmp = CreateCompatibleBitmap(window, w, h);
        let old_bmp = SelectObject(dc, bmp as HGDIOBJ);
        draw(hwnd, dc, &rc, g);
        BitBlt(window, 0, 0, w, h, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old_bmp);
        DeleteObject(bmp as HGDIOBJ);
        DeleteDC(dc);
        EndPaint(hwnd, &ps);
    }
}

/// The control's picture in `dc`, `rc` being its client area. Disabled, it is drawn as
/// µTorrent 2.2.1 draws it: white cells, grey legend swatches and grey text.
unsafe fn draw(hwnd: HWND, dc: HDC, rc: &RECT, g: &Grid) {
    unsafe {
        let enabled = IsWindowEnabled(hwnd) != 0;
        FillRect(dc, rc, GetSysColorBrush(COLOR_BTNFACE));
        let font = send(GetParent(hwnd), WM_GETFONT, 0, 0) as HFONT;
        let old_font = SelectObject(dc, font as HGDIOBJ);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, GetSysColor(if enabled { COLOR_BTNTEXT } else { COLOR_GRAYTEXT }));
        let l = Layout::new(rc.right, rc.bottom);
        for h in (0..HOURS).step_by(3) {
            let c = l.cell_rect(0, h);
            let r = RECT { left: c.left - l.cell_w, top: 0, right: c.left + 2 * l.cell_w, bottom: l.top };
            text(dc, &h.to_string(), &r, DT_CENTER);
        }
        // black grid lines, as µTorrent's
        let frame = CreateSolidBrush(0x0000_0000);
        let brushes = Mode::ALL.map(|m| CreateSolidBrush(color(m, enabled)));
        let brush = |m: Mode| brushes[Mode::ALL.iter().position(|&x| x == m).unwrap_or(0)];
        let white = brush(Mode::Off);
        for (day, name) in DAY_NAMES.iter().enumerate() {
            let c = l.cell_rect(day, 0);
            let r = RECT { left: 0, top: c.top, right: l.label_w - 2, bottom: c.bottom };
            text(dc, lang::text(*name), &r, DT_LEFT);
            for hour in 0..HOURS {
                let c = l.cell_rect(day, hour);
                FillRect(dc, &c, if enabled { brush(g.table[day * HOURS + hour]) } else { white });
                FrameRect(dc, &c, frame);
            }
        }
        // legend: a swatch and the mode's name, four to the line in equal slots; when a
        // translation does not fit its slot, packed from the left edge instead
        let widths = Mode::ALL.map(|m| l.line_h + text_width(dc, m.label()));
        let n = Mode::ALL.len() as i32;
        let slot = (rc.right - l.label_w) / n;
        let packed = widths.iter().any(|&w| w > slot);
        let gap = if packed { ((rc.right - widths.iter().sum::<i32>()) / (n - 1)).max(4) } else { 0 };
        let mut x = if packed { 0 } else { l.label_w };
        for (m, w) in Mode::ALL.into_iter().zip(widths) {
            let sw = RECT {
                left: x,
                top: l.legend_y + 2,
                right: x + l.line_h - 4,
                bottom: l.legend_y + l.line_h - 2,
            };
            FillRect(dc, &sw, brush(m));
            FrameRect(dc, &sw, frame);
            let next = if packed { x + w + gap } else { x + slot };
            let r = RECT { left: sw.right + 4, top: l.legend_y, right: next, bottom: l.legend_y + l.line_h };
            text(dc, m.label(), &r, DT_LEFT | DT_END_ELLIPSIS);
            x = next;
        }
        if let Some(cell) = g.hover {
            let y = l.legend_y + l.line_h;
            let r = RECT { left: l.label_w, top: y, right: rc.right, bottom: y + l.line_h };
            text(dc, &hover_text(&g.table, cell), &r, DT_LEFT);
        }
        DeleteObject(frame as HGDIOBJ);
        for b in brushes {
            DeleteObject(b as HGDIOBJ);
        }
        SelectObject(dc, old_font);
    }
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let g = Box::new(Grid { table: [Mode::Full; CELLS], painting: None, hover: None });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(g) as isize);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Grid;
        if p.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(p));
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let g = &mut *p;
        match msg {
            SET => {
                g.table = *(lp as *const [Mode; CELLS]);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            GET => {
                *(lp as *mut [Mode; CELLS]) = g.table;
                0
            }
            WM_PAINT => {
                paint(hwnd, g);
                0
            }
            // the bitmap covers the client area: clearing it first is what flickered
            WM_ERASEBKGND => 1,
            WM_ENABLE => {
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_from_lparam(lp);
                if let Some(cell) = layout(hwnd).hit(x, y) {
                    let m = g.table[cell].next();
                    g.table[cell] = m;
                    g.painting = Some((m, cell));
                    g.hover = Some(cell);
                    SetCapture(hwnd);
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_from_lparam(lp);
                let cell = layout(hwnd).hit(x, y);
                let mut dirty = cell != g.hover;
                g.hover = cell;
                if let (Some(c), Some((m, from))) = (cell, g.painting) {
                    for i in drag_line(from, c) {
                        dirty |= g.table[i] != m;
                        g.table[i] = m;
                    }
                    g.painting = Some((m, c));
                }
                if dirty {
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_LBUTTONUP => {
                g.painting = None;
                ReleaseCapture();
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_hit_where_they_are_drawn() {
        let l = Layout::new(620, 260);
        for (day, hour) in [(0, 0), (3, 12), (6, 23)] {
            let r = l.cell_rect(day, hour);
            assert_eq!(l.hit(r.left + 1, r.top + 1), Some(day * HOURS + hour));
        }
        assert_eq!(l.hit(1, 100), None);
        assert_eq!(l.hit(300, 1), None);
        let last = l.cell_rect(6, 23);
        assert_eq!(l.hit(last.right + 2, last.top + 1), None);
        assert_eq!(l.hit(last.left + 1, last.bottom + 2), None);
        assert!(l.legend_y + 2 * l.line_h <= 260);
    }
}
