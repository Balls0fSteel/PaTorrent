//! Report-style list views (the torrent list and the detail tabs) and their columns.

use std::mem::zeroed;

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::UI::Controls::{
    HDF_SORTDOWN, HDF_SORTUP, HDI_FORMAT, HDITEMW, HDM_GETITEMW, HDM_SETITEMW, LVCF_FMT, LVCF_SUBITEM,
    LVCF_TEXT, LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVIR_BOUNDS, LVIR_LABEL, LVIS_FOCUSED,
    LVIS_SELECTED, LVITEMW, LVM_DELETECOLUMN, LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNWIDTH, LVM_GETHEADER,
    LVM_GETNEXTITEM, LVM_GETSUBITEMRECT, LVM_INSERTCOLUMNW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT,
    LVM_SETITEMSTATE, LVNI_FOCUSED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_EX_HEADERDRAGDROP, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT, LVS_SHAREIMAGELISTS,
    LVS_SHOWSELALWAYS, LVSICF_NOINVALIDATEALL, LVSICF_NOSCROLL, WC_LISTVIEWW,
};
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

/// Selected row indices.
pub unsafe fn selected(lv: HWND) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = -1isize;
    loop {
        i = unsafe { send(lv, LVM_GETNEXTITEM, i as usize, LVNI_SELECTED as isize) };
        if i < 0 {
            return out;
        }
        out.push(i as usize);
    }
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
