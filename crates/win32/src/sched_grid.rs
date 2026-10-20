//! Preferences > Scheduler's table: 7 days × 24 hours, one colored cell per hour, as in
//! µTorrent 2.2.1. A click moves a cell to the next mode; dragging paints the cells passed over
//! with that same mode. The legend and the hovered hour's text are drawn below the cells.
//!
//! A window class of its own (`PaTorrentSchedGrid`) for the dialog template; the dialog writes
//! and reads the table with [`SET`] and [`GET`].

use std::mem::zeroed;
use std::ptr::null;

use patorrent_frontend::lang;
use patorrent_frontend::scheduler::{CELLS, DAY_NAMES, DAYS, HOURS, Mode};
use patorrent_frontend::tr;
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_BTNFACE, COLOR_BTNTEXT, CreateSolidBrush, DT_CENTER, DT_LEFT, DT_SINGLELINE,
    DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect, GetSysColor, GetSysColorBrush, HFONT,
    HGDIOBJ, InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GWLP_USERDATA, GetClientRect, GetParent, GetWindowLongPtrW, IDC_ARROW, LoadCursorW,
    RegisterClassExW, SetWindowLongPtrW, WM_GETFONT, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE,
    WM_NCDESTROY, WM_PAINT, WM_USER, WNDCLASSEXW,
};

use crate::util::{point_from_lparam, send, wide};

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

/// Cell colors (BGR), µTorrent's: green, light green, white, blue.
fn color(m: Mode) -> COLORREF {
    match m {
        Mode::Full => 0x0040b040,
        Mode::Limited => 0x00a0e8a0,
        Mode::Off => 0x00ffffff,
        Mode::SeedOnly => 0x00e0a060,
    }
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

/// The cells on the line from `from` to `to` (without `from`), so a fast drag leaves no gaps.
fn line(from: usize, to: usize) -> Vec<usize> {
    let (d0, h0, d1, h1) =
        ((from / HOURS) as f32, (from % HOURS) as f32, (to / HOURS) as f32, (to % HOURS) as f32);
    let n = (d1 - d0).abs().max((h1 - h0).abs()) as usize;
    (1..=n)
        .map(|k| {
            let t = k as f32 / n as f32;
            ((d0 + (d1 - d0) * t).round() as usize) * HOURS + (h0 + (h1 - h0) * t).round() as usize
        })
        .collect()
}

unsafe fn layout(hwnd: HWND) -> Layout {
    let mut rc: RECT = unsafe { zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    Layout::new(rc.right, rc.bottom)
}

unsafe fn text(dc: windows_sys::Win32::Graphics::Gdi::HDC, s: &str, rc: &RECT, flags: u32) {
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut r = *rc;
    unsafe { DrawTextW(dc, w.as_ptr(), w.len() as i32, &mut r, flags | DT_SINGLELINE | DT_VCENTER) };
}

fn hover_text(g: &Grid, cell: usize) -> String {
    let (day, hour) = (cell / HOURS, cell % HOURS);
    tr!(
        SchedHover,
        day = lang::text(DAY_NAMES[day]),
        from = format!("{hour:02}"),
        to = format!("{:02}", (hour + 1) % 24),
        mode = g.table[cell].label()
    )
}

unsafe fn paint(hwnd: HWND, g: &Grid) {
    unsafe {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = zeroed();
        GetClientRect(hwnd, &mut rc);
        FillRect(dc, &rc, GetSysColorBrush(COLOR_BTNFACE));
        let font = send(GetParent(hwnd), WM_GETFONT, 0, 0) as HFONT;
        let old_font = SelectObject(dc, font as HGDIOBJ);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, GetSysColor(COLOR_BTNTEXT));
        let l = Layout::new(rc.right, rc.bottom);
        for h in (0..HOURS).step_by(3) {
            let c = l.cell_rect(0, h);
            let r = RECT { left: c.left - l.cell_w, top: 0, right: c.left + 2 * l.cell_w, bottom: l.top };
            text(dc, &h.to_string(), &r, DT_CENTER);
        }
        let frame = CreateSolidBrush(0x00808080);
        let brushes = Mode::ALL.map(|m| CreateSolidBrush(color(m)));
        let brush = |m: Mode| brushes[Mode::ALL.iter().position(|&x| x == m).unwrap_or(0)];
        for (day, name) in DAY_NAMES.iter().enumerate() {
            let c = l.cell_rect(day, 0);
            let r = RECT { left: 0, top: c.top, right: l.label_w - 2, bottom: c.bottom };
            text(dc, lang::text(*name), &r, DT_LEFT);
            for hour in 0..HOURS {
                let c = l.cell_rect(day, hour);
                FillRect(dc, &c, brush(g.table[day * HOURS + hour]));
                FrameRect(dc, &c, frame);
            }
        }
        // legend: a swatch and the mode's name, four to the line
        let slot = (rc.right - l.label_w) / Mode::ALL.len() as i32;
        for (i, m) in Mode::ALL.into_iter().enumerate() {
            let x = l.label_w + i as i32 * slot;
            let sw = RECT {
                left: x,
                top: l.legend_y + 2,
                right: x + l.line_h - 4,
                bottom: l.legend_y + l.line_h - 2,
            };
            FillRect(dc, &sw, brush(m));
            FrameRect(dc, &sw, frame);
            let r =
                RECT { left: sw.right + 4, top: l.legend_y, right: x + slot, bottom: l.legend_y + l.line_h };
            text(dc, m.label(), &r, DT_LEFT);
        }
        if let Some(cell) = g.hover {
            let y = l.legend_y + l.line_h;
            let r = RECT { left: l.label_w, top: y, right: rc.right, bottom: y + l.line_h };
            text(dc, &hover_text(g, cell), &r, DT_LEFT);
        }
        DeleteObject(frame as HGDIOBJ);
        for b in brushes {
            DeleteObject(b as HGDIOBJ);
        }
        SelectObject(dc, old_font);
        EndPaint(hwnd, &ps);
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
                    for i in line(from, c) {
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

    #[test]
    fn a_fast_drag_fills_the_cells_between() {
        assert_eq!(line(25, 28), [26, 27, 28]);
        assert_eq!(line(28, 25), [27, 26, 25]);
        assert_eq!(line(5, 5), Vec::<usize>::new());
        assert_eq!(line(0, 2 * HOURS + 2), [HOURS + 1, 2 * HOURS + 2]);
        assert_eq!(line(0, 2 * HOURS), [HOURS, 2 * HOURS]);
    }

    #[test]
    fn the_hover_text_names_the_hour() {
        let mut g = Grid { table: [Mode::Full; CELLS], painting: None, hover: None };
        g.table[6 * HOURS + 23] = Mode::SeedOnly;
        assert_eq!(hover_text(&g, 6 * HOURS + 23), "Sun 23:00 - 00:00: Seeding only");
        assert_eq!(hover_text(&g, 9), "Mon 09:00 - 10:00: Full speed");
    }
}
