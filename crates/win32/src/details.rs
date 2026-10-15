//! The detail tabs under the torrent list: General, Trackers, Peers, Pieces, Files, Speed and
//! Logger. List tabs are owner-data list views; General, Pieces and Speed are painted panels.

use std::collections::VecDeque;
use std::sync::Arc;

use libretorrent_core::{AnnounceEntry, Bitfield, FileEntry, PeerInfo, TorrentId, TorrentStatus};
use libretorrent_frontend::detail::{self, ColumnDef};
use libretorrent_frontend::{columns::RowExtra, fmt, status::status_text};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Foundation::{COLORREF, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_BTNSHADOW, COLOR_GRAYTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreatePen,
    CreateSolidBrush, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    DeleteObject, DrawTextW, FillRect, FrameRect, GetSysColor, GetSysColorBrush, HDC, HFONT, LineTo,
    MoveToEx, PS_SOLID, Polyline, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};

use crate::util::{scale, wide};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    General,
    Trackers,
    Peers,
    Pieces,
    Files,
    Speed,
    Logger,
}

impl Page {
    pub const ALL: [Page; 7] =
        [Page::General, Page::Trackers, Page::Peers, Page::Pieces, Page::Files, Page::Speed, Page::Logger];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Trackers => "Trackers",
            Page::Peers => "Peers",
            Page::Pieces => "Pieces",
            Page::Files => "Files",
            Page::Speed => "Speed",
            Page::Logger => "Logger",
        }
    }

    pub fn index(self) -> usize {
        Page::ALL.iter().position(|&p| p == self).unwrap_or(0)
    }

    /// List view pages; the others are painted.
    pub fn is_list(self) -> bool {
        matches!(self, Page::Trackers | Page::Peers | Page::Files | Page::Logger)
    }

    pub fn columns(self) -> &'static [ColumnDef] {
        const LOG: [ColumnDef; 1] = [("Message", 900, false)];
        match self {
            Page::Trackers => &detail::TRACKER_COLUMNS,
            Page::Peers => &detail::PEER_COLUMNS,
            Page::Files => &detail::FILE_COLUMNS,
            Page::Logger => &LOG,
            _ => &[],
        }
    }
}

pub const LOG_LINES: usize = 1000;
/// Seconds of speed history.
pub const SPEED_SAMPLES: usize = 600;

/// What the detail tabs show for the selected torrent, plus the session-wide log and speeds.
#[derive(Default)]
pub struct Detail {
    pub id: Option<TorrentId>,
    pub peers: Vec<PeerInfo>,
    pub trackers: Vec<AnnounceEntry>,
    pub files: Option<Arc<[FileEntry]>>,
    /// Indices into `files` without pad files.
    pub file_rows: Vec<usize>,
    pub file_done: Vec<u64>,
    pub have: Option<Bitfield>,
    pub availability: Vec<u32>,
    pub log: VecDeque<String>,
    /// (download, upload) payload bytes per second, oldest first.
    pub speed: VecDeque<(u32, u32)>,
}

impl Detail {
    /// Switches to another torrent, dropping the previous one's data.
    pub fn select(&mut self, id: Option<TorrentId>) {
        if self.id != id {
            self.id = id;
            self.peers.clear();
            self.trackers.clear();
            self.files = None;
            self.file_rows.clear();
            self.file_done.clear();
            self.have = None;
            self.availability.clear();
        }
    }

    pub fn set_files(&mut self, files: Arc<[FileEntry]>) {
        self.file_rows = (0..files.len()).filter(|&i| !files[i].pad_file).collect();
        self.files = Some(files);
    }

    pub fn push_log(&mut self, line: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    pub fn push_speed(&mut self, down: u32, up: u32) {
        if self.speed.len() == SPEED_SAMPLES {
            self.speed.pop_front();
        }
        self.speed.push_back((down, up));
    }

    pub fn rows(&self, page: Page) -> usize {
        match page {
            Page::Trackers => self.trackers.len(),
            Page::Peers => self.peers.len(),
            Page::Files => self.file_rows.len(),
            Page::Logger => self.log.len(),
            _ => 0,
        }
    }

    pub fn cell(&self, page: Page, row: usize, col: usize) -> String {
        match page {
            Page::Trackers => self.trackers.get(row).map(|t| detail::tracker_cell(t, col)),
            Page::Peers => self.peers.get(row).map(|p| detail::peer_cell(p, col)),
            Page::Files => self.file_rows.get(row).and_then(|&i| {
                let f = &self.files.as_ref()?[i];
                Some(detail::file_cell(f, self.file_done.get(i).copied(), col))
            }),
            Page::Logger => self.log.get(row).cloned(),
            _ => None,
        }
        .unwrap_or_default()
    }

    /// File index (into the torrent's file list) of a Files tab row.
    pub fn file_index(&self, row: usize) -> Option<u32> {
        self.file_rows.get(row).map(|&i| i as u32)
    }
}

fn rgb(c: u32) -> COLORREF {
    ((c & 0xff) << 16) | (c & 0xff00) | ((c >> 16) & 0xff)
}

const BAR_BLUE: u32 = 0x3c78d8;
const BAR_GREEN: u32 = 0x4bb543;
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
    unsafe { DrawTextW(hdc, w.as_ptr(), -1, &mut r, flags | DT_SINGLELINE | DT_NOPREFIX | DT_VCENTER) };
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
        unsafe { hint(c, "Select a torrent to see its details.") };
        return;
    };
    let pad = c.s(8);
    let line = c.s(18);
    let label_w = c.s(110);
    let left = c.rc.left + pad;
    let right = c.rc.right - pad;
    let mut y = c.rc.top + pad;
    unsafe {
        // Downloaded and availability bars.
        let pieces = c.detail.have.as_ref().map_or(st.pieces_total as usize, |h| h.len());
        let bars: [(&str, String); 2] = [
            ("Downloaded:", fmt::percent(st.progress_ppm)),
            ("Availability:", fmt::availability(st.distributed_copies)),
        ];
        for (i, (label, value)) in bars.iter().enumerate() {
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
        y = section(c, "Transfer", y);
        let remaining = st.total_wanted.saturating_sub(st.total_wanted_done);
        let grid: [[(&str, String); 3]; 4] = [
            [
                ("Time Elapsed:", fmt::duration(st.active_duration)),
                ("Remaining:", if st.is_finished { String::new() } else { fmt::eta(st.eta()) }),
                ("Wasted:", fmt::size(st.total_failed_bytes + st.total_redundant_bytes)),
            ],
            [
                ("Downloaded:", fmt::size(st.all_time_download)),
                ("Uploaded:", fmt::size(st.all_time_upload)),
                (
                    "Seeds:",
                    format!("{} of {} connected", st.num_seeds, st.num_complete.unwrap_or(st.list_seeds)),
                ),
            ],
            [
                ("Download Speed:", fmt::speed_always(st.download_payload_rate)),
                ("Upload Speed:", fmt::speed_always(st.upload_payload_rate)),
                (
                    "Peers:",
                    format!(
                        "{} of {} connected",
                        st.num_peers.saturating_sub(st.num_seeds),
                        st.num_incomplete.unwrap_or(st.list_peers.saturating_sub(st.list_seeds))
                    ),
                ),
            ],
            [
                ("Down Limit:", limit(st.download_limit)),
                ("Up Limit:", limit(st.upload_limit)),
                ("Share Ratio:", fmt::ratio(st.ratio())),
            ],
        ];
        let col_w = (right - left) / 3;
        for row in &grid {
            for (i, (label, value)) in row.iter().enumerate() {
                let x0 = left + col_w * i as i32;
                pair(c, x0, y, col_w, label, value);
            }
            y += line;
        }
        pair(c, left, y, right - left, "Status:", &status_text(st, x.user_paused));
        y += line + c.s(6);

        y = section(c, "General", y);
        let pieces_text = if st.piece_length > 0 {
            format!("{} x {} (have {})", st.pieces_total, fmt::size(st.piece_length as u64), st.num_pieces)
        } else {
            String::new()
        };
        let rows: [(&str, String, &str, String); 3] = [
            ("Save As:", st.save_path.clone(), "Remaining:", fmt::size(remaining)),
            ("Total Size:", fmt::size(st.total), "Added On:", fmt::date_time(st.added_time)),
            ("Pieces:", pieces_text, "Completed On:", fmt::date_time(st.completed_time)),
        ];
        let half = (right - left) * 2 / 3;
        for (l1, v1, l2, v2) in &rows {
            pair(c, left, y, half, l1, v1);
            pair(c, left + half, y, right - left - half, l2, v2);
            y += line;
        }
        pair(c, left, y, right - left, "Hash:", &st.info_hashes.key_hex().to_uppercase());
    }
}

fn limit(v: u32) -> String {
    if v == 0 { "∞".to_owned() } else { fmt::speed_always(v) }
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

unsafe fn pieces(c: &PaintCtx<'_>) {
    let d = c.detail;
    let Some(have) = &d.have else {
        let msg = if c.status.is_some() { "Waiting for piece information..." } else { "Select a torrent." };
        unsafe { hint(c, msg) };
        return;
    };
    let n = have.len();
    if n == 0 {
        return;
    }
    let pad = c.s(8);
    let cell = c.s(9);
    let gap = c.s(1).max(1);
    let cols = ((c.rc.right - c.rc.left - 2 * pad) / (cell + gap)).max(1) as usize;
    let header = c.s(22);
    unsafe {
        let r = RECT {
            left: c.rc.left + pad,
            top: c.rc.top + c.s(4),
            right: c.rc.right - pad,
            bottom: c.rc.top + header,
        };
        let s = format!("{} pieces, {} done. Darker squares are pieces more peers have.", n, have.count());
        text(c.hdc, &s, &r, DT_LEFT | DT_END_ELLIPSIS);
        let max_rows = ((c.rc.bottom - c.rc.top - header - pad) / (cell + gap)).max(0) as usize;
        let shown = n.min(cols * max_rows);
        for i in 0..shown {
            let x = c.rc.left + pad + ((i % cols) as i32) * (cell + gap);
            let y = c.rc.top + header + ((i / cols) as i32) * (cell + gap);
            let rc = RECT { left: x, top: y, right: x + cell, bottom: y + cell };
            let color = if have.get(i) {
                BAR_BLUE
            } else {
                match d.availability.get(i).copied().unwrap_or(0) {
                    0 => 0xffffff,
                    1 => 0xd8e8d0,
                    2 | 3 => 0xa8d29a,
                    _ => 0x6fb85c,
                }
            };
            fill(c.hdc, &rc, color);
            FrameRect(c.hdc, &rc, GetSysColorBrush(COLOR_BTNSHADOW));
        }
        if shown < n {
            let y = c.rc.bottom - c.s(18);
            let r = RECT { left: c.rc.left + pad, top: y, right: c.rc.right - pad, bottom: c.rc.bottom };
            text(c.hdc, &format!("... {} more", n - shown), &r, DT_RIGHT);
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
    let max = d.speed.iter().map(|&(a, b)| a.max(b)).max().unwrap_or(0).max(10 * 1024);
    // Round the scale up to 1, 2 or 5 × 10^n kB/s.
    let mut top;
    let mut base = 1024u64;
    'scale: loop {
        for m in [1, 2, 5] {
            top = base * m;
            if top >= max as u64 {
                break 'scale;
            }
        }
        base *= 10;
    }
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
            (format!("Download: {}", fmt::speed_always(down)), DOWN_COLOR),
            (format!("Upload: {}", fmt::speed_always(up)), UP_COLOR),
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
