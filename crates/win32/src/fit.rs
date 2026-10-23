//! Translated texts in fixed dialog layouts. A label, check box or button whose text no longer
//! fits is widened into the free space beside it, up to the next control on its row and the
//! group box or tab control (or dialog) around it. When a field follows on the row, the rest of the row moves
//! right into the room after it, and the widest field there may give up width down to a
//! minimum. Labels aligned right (`SS_RIGHT`, in front of a field) grow to the left first.
//! Last, the controls before it on the row close up: each gives up the width its own text (or,
//! for a field or a note of several lines, its minimum) does not need, or moves into free
//! space before it. What still does not fit stays clipped. Run once a dialog's texts are set (`WM_INITDIALOG`).
//! Coordinates are the dialog's own, so a mirrored (right-to-left) dialog fits the same way.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    DT_CALCRECT, DT_NOPREFIX, DT_SINGLELINE, DrawTextW, GetDC, HFONT, MapWindowPoints, ReleaseDC,
    SelectObject,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_3STATE, BS_AUTO3STATE, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_CHECKBOX, BS_DEFPUSHBUTTON,
    BS_GROUPBOX, BS_PUSHBUTTON, BS_RADIOBUTTON, BS_TYPEMASK, GW_CHILD, GW_HWNDNEXT, GWL_ID, GWL_STYLE,
    GetClassNameW, GetClientRect, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowPos, WM_GETFONT,
};

use crate::util::scale;

// Static control styles (in `System::SystemServices`, a feature we do not otherwise need).
const SS_LEFT: u32 = 0;
const SS_RIGHT: u32 = 2;
const SS_LEFTNOWORDWRAP: u32 = 0xc;
const SS_TYPEMASK: u32 = 0x1f;
const SS_NOPREFIX: u32 = 0x80;
/// `SS_ENDELLIPSIS`, `SS_PATHELLIPSIS` and `SS_WORDELLIPSIS` together.
const SS_ELLIPSISMASK: u32 = 0xc000;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Label {
        right: bool,
        prefix: bool,
    },
    Check,
    Push,
    /// A group box or a tab control: the box the controls inside it fit in.
    Group,
    /// An edit or a drop-down, which may give up width down to `min` pixels.
    Field {
        min: i32,
    },
    Other,
}

struct Ctl {
    hwnd: HWND,
    kind: Kind,
    /// The set it is laid out with; `None` (controls outside Preferences' pages) blocks all.
    group: Option<i32>,
    rect: RECT,
    /// The width its text needs (labels, check boxes, buttons).
    need: Option<i32>,
}

/// Fits every child of `dlg`; `group` maps a control id to the page it belongs to when pages
/// share the dialog (Preferences), so a hidden page's controls do not get in the way.
pub fn fit_texts(dlg: HWND, group: impl Fn(u16) -> Option<i32>) {
    unsafe {
        let dpi = GetDpiForWindow(dlg).max(96);
        let mut ctls = Vec::new();
        let mut h = GetWindow(dlg, GW_CHILD);
        while !h.is_null() {
            let id = GetWindowLongW(h, GWL_ID) as u16;
            let kind = kind(h, dpi);
            let need = match kind {
                Kind::Label { .. } | Kind::Check | Kind::Push => needed_width(h, kind, dpi),
                _ => None,
            };
            ctls.push(Ctl { hwnd: h, kind, group: group(id), rect: rect_in(dlg, h), need });
            h = GetWindow(h, GW_HWNDNEXT);
        }
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(dlg, &mut client);
        let (margin, gap) = (scale(6, dpi), scale(3, dpi));
        for i in 0..ctls.len() {
            let (kind, own) = (ctls[i].kind, ctls[i].rect);
            if matches!(kind, Kind::Group | Kind::Field { .. } | Kind::Other) {
                continue;
            }
            let Some(need) = ctls[i].need else { continue };
            if need <= own.right - own.left {
                continue;
            }
            let (mut box_left, mut box_right) = (client.left + margin, client.right - margin);
            let mut left_limit = box_left;
            let mut row = Vec::new(); // the controls after it on its row
            let mut before = Vec::new(); // and before it
            for (j, o) in ctls.iter().enumerate() {
                if j == i || !(o.group.is_none() || ctls[i].group.is_none() || o.group == ctls[i].group) {
                    continue;
                }
                let r = &o.rect;
                if o.kind == Kind::Group {
                    if r.left <= own.left
                        && r.right >= own.right
                        && r.top <= own.top
                        && r.bottom >= own.bottom
                    {
                        box_left = box_left.max(r.left + margin);
                        box_right = box_right.min(r.right - margin);
                    }
                    continue;
                }
                if r.top >= own.bottom || r.bottom <= own.top {
                    continue; // another row
                }
                if r.left >= own.right - 1 {
                    row.push(j);
                } else if r.right <= own.left + 1 {
                    left_limit = left_limit.max(r.right + gap);
                    before.push(j);
                }
            }
            before.sort_by_key(|&j| std::cmp::Reverse(ctls[j].rect.right));
            let left_limit = left_limit.max(box_left);
            let right_limit = row.iter().map(|&j| ctls[j].rect.left - gap).fold(box_right, i32::min);
            let line = own.bottom - own.top;
            let mut r = own;
            let short = |r: &RECT| need - (r.right - r.left);
            let free_left = |r: &mut RECT| r.left -= short(r).clamp(0, (r.left - left_limit).max(0));
            let free_right = |r: &mut RECT| r.right += short(r).clamp(0, (right_limit - r.right).max(0));
            match kind {
                // in front of a field: grow away from it first
                Kind::Label { right: true, .. } => {
                    free_left(&mut r);
                    free_right(&mut r);
                }
                Kind::Push => {
                    free_right(&mut r);
                    free_left(&mut r);
                }
                // the first on its row keeps its place (check boxes line up down the page)
                _ => {
                    free_right(&mut r);
                    if !before.is_empty() {
                        free_left(&mut r);
                    }
                }
            }
            if short(&r) > 0 {
                r.right += make_room(&mut ctls, &row, short(&r), box_right, line);
            }
            if short(&r) > 0 {
                r.left -= close_up(&mut ctls, &before, short(&r), r.left - gap, box_left, gap, line);
            }
            if r.left != own.left || r.right != own.right {
                place(&mut ctls[i], r);
            }
        }
    }
}

/// Moves `row` (controls on one row, left to right after a text that is `short` pixels too
/// narrow) right by up to `short`: into the room before `box_right`, then into what the
/// widest field among them can give up. Returns how far it moved them. A tall control on the
/// row other than a field (a list) keeps the row where it is.
unsafe fn make_room(ctls: &mut [Ctl], row: &[usize], short: i32, box_right: i32, line: i32) -> i32 {
    if row.is_empty() || row.iter().any(|&j| is_wall(&ctls[j], line)) {
        return 0;
    }
    let row_right = row.iter().map(|&j| ctls[j].rect.right).max().unwrap_or(box_right);
    let slack = (box_right - row_right).max(0);
    let field = row
        .iter()
        .copied()
        .filter(|&j| matches!(ctls[j].kind, Kind::Field { .. }))
        .max_by_key(|&j| spare(&ctls[j]))
        .filter(|&j| spare(&ctls[j]) > 0);
    let shift = short.min(slack + field.map_or(0, |j| spare(&ctls[j])));
    if shift <= 0 {
        return 0;
    }
    // what the field gives up, taken back from it and from everything after it
    let excess = (shift - slack).max(0);
    let field_left = field.map(|j| ctls[j].rect.left);
    for &j in row {
        let mut r = ctls[j].rect;
        r.left += shift;
        r.right += shift;
        if Some(j) == field {
            r.right -= excess;
        } else if field_left.is_some_and(|f| r.left - shift > f) {
            r.left -= excess;
            r.right -= excess;
        }
        unsafe { place(&mut ctls[j], r) };
    }
    shift
}

/// Frees up to `short` pixels before `edge` (the right edge the nearest of `before` may keep;
/// `before` is nearest first): each control gives up its spare width, and moves left into the
/// free space before it, passing what is still missing to the next. Lists stay put.
/// Returns how much was freed.
unsafe fn close_up(
    ctls: &mut [Ctl],
    before: &[usize],
    short: i32,
    edge: i32,
    box_left: i32,
    gap: i32,
    line: i32,
) -> i32 {
    // a dry run says how much can be had; the second run moves that much
    let plan = |ctls: &[Ctl], short: i32| {
        let (mut need, mut edge, mut moves) = (short, edge, Vec::new());
        for (n, &k) in before.iter().enumerate() {
            let c = &ctls[k];
            if need <= 0 || is_wall(c, line) {
                break;
            }
            // what lies between it and the edge is free already
            need -= (edge - c.rect.right).max(0);
            if need <= 0 {
                break;
            }
            let shrink = spare(c).min(need);
            let wall = before.get(n + 1).map_or(box_left, |&b| ctls[b].rect.right + gap);
            let slide = (need - shrink).min((c.rect.left - wall).max(0));
            moves.push((k, slide, shrink));
            need -= shrink + slide;
            edge = c.rect.left - slide - gap;
        }
        (short - need.max(0), moves)
    };
    let (can, _) = plan(ctls, short);
    if can <= 0 {
        return 0;
    }
    let (got, moves) = plan(ctls, can);
    // farthest first: what each frees, the ones nearer the text move left by too
    let mut carry = 0;
    for (k, slide, shrink) in moves.into_iter().rev() {
        let mut r = ctls[k].rect;
        r.left -= slide + carry;
        r.right -= slide + shrink + carry;
        carry += slide + shrink;
        unsafe { place(&mut ctls[k], r) };
    }
    got
}

/// A control taller than a couple of rows that is not a field (a list): rows do not move it.
fn is_wall(c: &Ctl, line: i32) -> bool {
    c.rect.bottom - c.rect.top > line * 5 / 2 && !matches!(c.kind, Kind::Field { .. })
}

/// How much width a control can give up: a field down to its minimum, a text down to what it
/// needs.
fn spare(c: &Ctl) -> i32 {
    let width = c.rect.right - c.rect.left;
    match (c.kind, c.need) {
        (Kind::Field { min }, _) => (width - min).max(0),
        (Kind::Label { .. } | Kind::Check | Kind::Push, Some(need)) => (width - need).max(0),
        _ => 0,
    }
}

unsafe fn place(c: &mut Ctl, r: RECT) {
    let flags = SWP_NOZORDER | SWP_NOACTIVATE;
    unsafe { SetWindowPos(c.hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, flags) };
    c.rect = r;
}

/// `h`'s rectangle in `dlg`'s client coordinates (mirroring included).
unsafe fn rect_in(dlg: HWND, h: HWND) -> RECT {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut r);
        MapWindowPoints(null_mut(), dlg, &mut r as *mut RECT as *mut POINT, 2);
        r
    }
}

unsafe fn kind(h: HWND, dpi: u32) -> Kind {
    unsafe {
        let mut class = [0u16; 32];
        let n = GetClassNameW(h, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        let style = GetWindowLongW(h, GWL_STYLE) as u32;
        if class.eq_ignore_ascii_case("Static") {
            let prefix = style & SS_NOPREFIX == 0;
            // a value that shows "..." for what does not fit (a torrent's comment) is never
            // widened to its text; like a field, it gives width to a label on its row
            if style & SS_ELLIPSISMASK != 0 {
                return Kind::Field { min: scale(48, dpi) };
            }
            // a note of several lines wraps its text, so it can be narrower
            if style & SS_TYPEMASK == SS_LEFT && is_tall(h) {
                return Kind::Field { min: scale(90, dpi) };
            }
            return match style & SS_TYPEMASK {
                t if t == SS_LEFT || t == SS_LEFTNOWORDWRAP => Kind::Label { right: false, prefix },
                t if t == SS_RIGHT => Kind::Label { right: true, prefix },
                _ => Kind::Other,
            };
        }
        if class.eq_ignore_ascii_case("Button") {
            return match (style & BS_TYPEMASK as u32) as i32 {
                t if t == BS_PUSHBUTTON || t == BS_DEFPUSHBUTTON => Kind::Push,
                t if [
                    BS_CHECKBOX,
                    BS_AUTOCHECKBOX,
                    BS_3STATE,
                    BS_AUTO3STATE,
                    BS_RADIOBUTTON,
                    BS_AUTORADIOBUTTON,
                ]
                .contains(&t) =>
                {
                    Kind::Check
                }
                t if t == BS_GROUPBOX => Kind::Group,
                _ => Kind::Other,
            };
        }
        if class.eq_ignore_ascii_case("SysTabControl32") {
            return Kind::Group;
        }
        // a field keeps room for about eight digits; a drop-down for a short word and its arrow
        if class.eq_ignore_ascii_case("Edit") {
            return Kind::Field { min: scale(48, dpi) };
        }
        if class.eq_ignore_ascii_case("ComboBox") {
            return Kind::Field { min: scale(72, dpi) };
        }
        Kind::Other
    }
}

/// Whether `h` is about two lines of its font tall or more.
unsafe fn is_tall(h: HWND) -> bool {
    unsafe {
        let dc = GetDC(h);
        let old = SelectObject(dc, SendMessageW(h, WM_GETFONT, 0, 0) as HFONT);
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let line = DrawTextW(dc, [b'A' as u16, b'g' as u16].as_ptr(), 2, &mut r, DT_CALCRECT | DT_SINGLELINE);
        SelectObject(dc, old);
        ReleaseDC(h, dc);
        let mut own = std::mem::zeroed::<RECT>();
        GetWindowRect(h, &mut own);
        own.bottom - own.top >= line * 7 / 4
    }
}

/// The width `h` needs for its text on one line, with the room its kind takes beside the text
/// (the check box, a button's edges); `None` for no text, or a label of several lines.
unsafe fn needed_width(h: HWND, kind: Kind, dpi: u32) -> Option<i32> {
    unsafe {
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return None;
        }
        let mut text = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(h, text.as_mut_ptr(), text.len() as i32).max(0) as usize;
        let text = &text[..n];
        if text.contains(&(b'\n' as u16)) {
            return None;
        }
        let dc = GetDC(h);
        let old = SelectObject(dc, SendMessageW(h, WM_GETFONT, 0, 0) as HFONT);
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let prefix = match kind {
            Kind::Label { prefix, .. } => prefix,
            _ => true,
        };
        let flags = DT_CALCRECT | DT_SINGLELINE | if prefix { 0 } else { DT_NOPREFIX };
        let line = DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut r, flags);
        SelectObject(dc, old);
        ReleaseDC(h, dc);
        // a label taller than about two lines wraps by design
        let mut own = std::mem::zeroed::<RECT>();
        GetWindowRect(h, &mut own);
        if matches!(kind, Kind::Label { .. }) && own.bottom - own.top > line * 3 / 2 {
            return None;
        }
        let extra = match kind {
            Kind::Check => scale(20, dpi),
            Kind::Push => scale(14, dpi),
            _ => scale(2, dpi),
        };
        Some(r.right - r.left + extra)
    }
}
