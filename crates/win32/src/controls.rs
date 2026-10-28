//! Report-style list views (the torrent list and the detail tabs) and their columns.

use std::mem::zeroed;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, CreateSolidBrush,
    DeleteObject, DrawFocusRect, FillRect, GetSysColor,
};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPOSTPAINT, CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDDS_SUBITEM, CDIS_FOCUS, CDIS_SELECTED,
    CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT, CDRF_NOTIFYSUBITEMDRAW, HDF_SORTDOWN,
    HDF_SORTUP, HDI_FORMAT, HDITEMW, HDM_GETITEMW, HDM_SETITEMW, LVCF_FMT, LVCF_SUBITEM, LVCF_TEXT,
    LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVIR_BOUNDS, LVIR_LABEL, LVIS_FOCUSED, LVIS_SELECTED,
    LVITEMW, LVM_DELETECOLUMN, LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNWIDTH, LVM_GETHEADER, LVM_GETITEMRECT,
    LVM_GETITEMSTATE, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_GETSUBITEMRECT, LVM_INSERTCOLUMNW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT, LVM_SETITEMSTATE, LVNI_FOCUSED, LVNI_SELECTED,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_HEADERDRAGDROP, LVS_EX_LABELTIP, LVS_OWNERDATA,
    LVS_REPORT, LVS_SHAREIMAGELISTS, LVS_SHOWSELALWAYS, LVSICF_NOINVALIDATEALL, LVSICF_NOSCROLL,
    NMLVCUSTOMDRAW, WC_LISTVIEWW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{WS_CHILD, WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE, WS_TABSTOP};

use crate::util::{child, send, wide};

/// Creates an owner-data report list view (the frontend keeps the rows).
pub unsafe fn create_report(parent: HWND, id: usize, header_drag: bool) -> HWND {
    unsafe {
        let lv = child(
            parent,
            WC_LISTVIEWW,
            "",
            // shared image lists: the state icons are the window's (see `app::apply_dpi`)
            WS_CHILD
                | WS_CLIPSIBLINGS
                | WS_TABSTOP
                | LVS_REPORT
                | LVS_OWNERDATA
                | LVS_SHOWSELALWAYS
                | LVS_SHAREIMAGELISTS,
            WS_EX_CLIENTEDGE,
            id,
        );
        let mut ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        if header_drag {
            ex |= LVS_EX_HEADERDRAGDROP;
        }
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        lv
    }
}

/// Replaces all columns. `cols` are (title, width in pixels, right-aligned).
pub unsafe fn set_columns(lv: HWND, cols: &[(&str, i32, bool)]) {
    unsafe {
        while send(lv, LVM_DELETECOLUMN, 0, 0) != 0 {}
        for (i, &(title, width, right)) in cols.iter().enumerate() {
            let mut text = wide(title);
            let mut c: LVCOLUMNW = zeroed();
            c.mask = LVCF_TEXT | LVCF_WIDTH | LVCF_FMT | LVCF_SUBITEM;
            c.fmt = if right { LVCFMT_RIGHT } else { LVCFMT_LEFT };
            c.cx = width;
            c.pszText = text.as_mut_ptr();
            c.iSubItem = i as i32;
            send(lv, LVM_INSERTCOLUMNW, i, &c as *const _ as LPARAM);
        }
    }
}

/// Column indices in display order (after header drag-and-drop) and their widths in pixels.
pub unsafe fn column_layout(lv: HWND, count: usize) -> Vec<(usize, i32)> {
    unsafe {
        let mut order = vec![0i32; count];
        if count == 0 || send(lv, LVM_GETCOLUMNORDERARRAY, count, order.as_mut_ptr() as LPARAM) == 0 {
            order = (0..count as i32).collect();
        }
        order.into_iter().map(|i| (i as usize, send(lv, LVM_GETCOLUMNWIDTH, i as usize, 0) as i32)).collect()
    }
}

/// Shows the sort arrow on column `col` (and clears the others).
pub unsafe fn set_sort_arrow(lv: HWND, count: usize, col: Option<usize>, descending: bool) {
    unsafe {
        let header = send(lv, LVM_GETHEADER, 0, 0) as HWND;
        for i in 0..count {
            let mut item: HDITEMW = zeroed();
            item.mask = HDI_FORMAT;
            send(header, HDM_GETITEMW, i, &mut item as *mut _ as LPARAM);
            item.fmt &= !(HDF_SORTUP | HDF_SORTDOWN);
            if col == Some(i) {
                item.fmt |= if descending { HDF_SORTDOWN } else { HDF_SORTUP };
            }
            send(header, HDM_SETITEMW, i, &item as *const _ as LPARAM);
        }
    }
}

pub unsafe fn header(lv: HWND) -> HWND {
    unsafe { send(lv, LVM_GETHEADER, 0, 0) as HWND }
}

/// Sets the row count without scrolling or repainting everything.
pub unsafe fn set_count(lv: HWND, n: usize) {
    unsafe {
        send(lv, LVM_SETITEMCOUNT, n, (LVSICF_NOINVALIDATEALL | LVSICF_NOSCROLL) as isize);
    }
}

/// How many rows are selected (without walking them).
pub unsafe fn selected_count(lv: HWND) -> usize {
    unsafe { send(lv, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as usize }
}

/// Selected row indices. Stops at the last selected row: asking past it makes an owner-data
/// list scan every remaining row (a millisecond at 100,000 rows).
pub unsafe fn selected(lv: HWND) -> Vec<usize> {
    let n = unsafe { selected_count(lv) };
    let mut out = Vec::with_capacity(n);
    let mut i = -1isize;
    while out.len() < n {
        i = unsafe { send(lv, LVM_GETNEXTITEM, i as usize, LVNI_SELECTED as isize) };
        if i < 0 {
            break;
        }
        out.push(i as usize);
    }
    out
}

/// The first selected row.
pub unsafe fn first_selected(lv: HWND) -> Option<usize> {
    if unsafe { selected_count(lv) } == 0 {
        return None;
    }
    let i = unsafe { send(lv, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize) };
    (i >= 0).then_some(i as usize)
}

pub unsafe fn is_selected(lv: HWND, row: usize) -> bool {
    unsafe { send(lv, LVM_GETITEMSTATE, row, LVIS_SELECTED as isize) as u32 & LVIS_SELECTED != 0 }
}

pub unsafe fn focused(lv: HWND) -> Option<usize> {
    let i = unsafe { send(lv, LVM_GETNEXTITEM, usize::MAX, LVNI_FOCUSED as isize) };
    (i >= 0).then_some(i as usize)
}

/// Sets or clears the selected and focused state of row `i` (`None` = every row).
pub unsafe fn set_state(lv: HWND, i: Option<usize>, selected: bool, focused: bool) {
    unsafe {
        let mut item: LVITEMW = zeroed();
        item.stateMask = LVIS_SELECTED | if focused { LVIS_FOCUSED } else { 0 };
        item.state = if selected { LVIS_SELECTED } else { 0 } | if focused { LVIS_FOCUSED } else { 0 };
        send(lv, LVM_SETITEMSTATE, i.unwrap_or(usize::MAX), &item as *const _ as LPARAM);
    }
}

/// The rectangle of one cell, in list coordinates.
pub unsafe fn cell_rect(lv: HWND, row: usize, sub: usize) -> RECT {
    unsafe {
        let mut rc: RECT = zeroed();
        rc.top = sub as i32;
        rc.left = if sub == 0 { LVIR_LABEL } else { LVIR_BOUNDS } as i32;
        send(lv, LVM_GETSUBITEMRECT, row, &mut rc as *mut _ as LPARAM);
        rc
    }
}

/// Whether every other row is shaded (Preferences > UI Settings "Alternate list background
/// color", `ui.alternate_rows`), for every list view painted through this module.
static ALTERNATE_ROWS: AtomicBool = AtomicBool::new(true);

pub fn set_alternate_rows(on: bool) {
    ALTERNATE_ROWS.store(on, Ordering::Relaxed);
}

/// The background of an unselected row: the window colour, or for every other row (when
/// shading is on) the window colour darkened a little, so it follows the system's colours.
fn row_background(row: usize) -> COLORREF {
    let window = unsafe { GetSysColor(COLOR_WINDOW) };
    if row % 2 == 0 || !ALTERNATE_ROWS.load(Ordering::Relaxed) {
        return window;
    }
    let shade = |c: u32| (c * 15 / 16) & 0xff;
    shade(window & 0xff) | (shade((window >> 8) & 0xff) << 8) | (shade((window >> 16) & 0xff) << 16)
}

/// Whether a custom-draw notification is a paint of the item. The list view also sends the
/// item stages with an empty `rc` and another device context while it measures a label for
/// its in-place tooltip (`LVS_EX_LABELTIP`, the mouse over a truncated name); that context can
/// be the window's own, outside any paint cycle, so a bar drawn there lands on the screen bare:
/// a row with no text until something repaints it. Nothing is painted for those.
pub fn is_paint(cd: &NMLVCUSTOMDRAW) -> bool {
    let rc = cd.nmcd.rc;
    rc.right > rc.left && rc.bottom > rc.top
}

/// Paints µTorrent's selection at `CDDS_ITEMPREPAINT`: one solid bar across the row
/// (`COLOR_HIGHLIGHT` while the list has the focus, `COLOR_BTNFACE` otherwise), not the theme's
/// per-cell highlights with the column padding left white between them. The item loses its
/// selected and focused states so the list view paints the cells' text over the bar with the
/// colours set here; the focus rectangle spans the row ([`selection_post_paint`]). An unselected
/// row gets its [`row_background`] the same way (the shaded bar spans the column padding too).
/// Returns the stage's result: the caller adds `CDRF_NOTIFYSUBITEMDRAW` when it draws cells of
/// its own.
pub unsafe fn selection_pre_paint(cd: &mut NMLVCUSTOMDRAW) -> LRESULT {
    unsafe {
        if !is_paint(cd) {
            return CDRF_DODEFAULT as LRESULT;
        }
        let lv = cd.nmcd.hdr.hwndFrom;
        let row = cd.nmcd.dwItemSpec;
        let selected = is_selected(lv, row);
        let bk = if selected { selection_colours(lv).0 } else { row_background(row) };
        let mut rc: RECT = zeroed();
        rc.left = LVIR_BOUNDS as i32;
        send(lv, LVM_GETITEMRECT, row, &mut rc as *mut _ as LPARAM);
        let brush = CreateSolidBrush(bk);
        FillRect(cd.nmcd.hdc, &rc, brush);
        DeleteObject(brush);
        selection_cell_colours(cd);
        if selected { CDRF_NOTIFYPOSTPAINT as LRESULT } else { CDRF_DODEFAULT as LRESULT }
    }
}

/// At `CDDS_ITEMPREPAINT | CDDS_SUBITEM`: the row's colours again (the list view restores the
/// item's state and colours for each cell): the selection's, or the unselected row's
/// background behind its text.
pub unsafe fn selection_cell_colours(cd: &mut NMLVCUSTOMDRAW) {
    unsafe {
        let lv = cd.nmcd.hdr.hwndFrom;
        let row = cd.nmcd.dwItemSpec;
        if !is_selected(lv, row) {
            cd.clrTextBk = row_background(row);
            return;
        }
        let (bk, text) = selection_colours(lv);
        cd.clrTextBk = bk;
        cd.clrText = text;
        cd.nmcd.uItemState &= !(CDIS_SELECTED | CDIS_FOCUS);
    }
}

/// At `CDDS_ITEMPOSTPAINT`: the focus rectangle around the whole row of the focused item while
/// the list has the focus.
pub unsafe fn selection_post_paint(cd: &NMLVCUSTOMDRAW) {
    unsafe {
        let lv = cd.nmcd.hdr.hwndFrom;
        let row = cd.nmcd.dwItemSpec;
        if !is_paint(cd) || GetFocus() != lv || focused(lv) != Some(row) {
            return;
        }
        let mut rc: RECT = zeroed();
        rc.left = LVIR_BOUNDS as i32;
        send(lv, LVM_GETITEMRECT, row, &mut rc as *mut _ as LPARAM);
        DrawFocusRect(cd.nmcd.hdc, &rc);
    }
}

/// (background, text) of a selected row.
unsafe fn selection_colours(lv: HWND) -> (COLORREF, COLORREF) {
    unsafe {
        if GetFocus() == lv {
            (GetSysColor(COLOR_HIGHLIGHT), GetSysColor(COLOR_HIGHLIGHTTEXT))
        } else {
            (GetSysColor(COLOR_BTNFACE), GetSysColor(COLOR_BTNTEXT))
        }
    }
}

/// The whole custom-draw handler of a list with no cells of its own: the selection bar and
/// the focus rectangle.
pub unsafe fn selection_custom_draw(cd: &mut NMLVCUSTOMDRAW) -> LRESULT {
    unsafe {
        match cd.nmcd.dwDrawStage {
            CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as LRESULT,
            CDDS_ITEMPREPAINT => selection_pre_paint(cd) | CDRF_NOTIFYSUBITEMDRAW as LRESULT,
            s if s == CDDS_ITEMPREPAINT | CDDS_SUBITEM => {
                selection_cell_colours(cd);
                CDRF_DODEFAULT as LRESULT
            }
            CDDS_ITEMPOSTPAINT => {
                selection_post_paint(cd);
                CDRF_DODEFAULT as LRESULT
            }
            _ => CDRF_DODEFAULT as LRESULT,
        }
    }
}
