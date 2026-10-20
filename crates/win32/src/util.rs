//! Small Win32 helpers: UTF-16 strings, DPI scaling, child window creation, paths.

use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetWindowTextLengthW, GetWindowTextW, HMENU, SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE,
};

/// `flags` when the language is written right to left, else 0: the extra flags message boxes
/// (`MB_RTLREADING | MB_RIGHT`), drawn text (`DT_RTLREADING`) and popup menus
/// (`TPM_LAYOUTRTL`) take then. Windows themselves are mirrored once, at start
/// (`SetProcessDefaultLayout` in `app::run`).
pub fn rtl(flags: u32) -> u32 {
    if patorrent_frontend::lang::right_to_left() { flags } else { 0 }
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Up to the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string.
pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

/// Copies `s` into a fixed buffer, truncated and NUL-terminated (list view callbacks).
///
/// # Safety
/// `dst` must be valid for `cap` writes.
pub unsafe fn copy_to(dst: *mut u16, cap: i32, s: &str) {
    if dst.is_null() || cap <= 0 {
        return;
    }
    let cap = cap as usize;
    let mut n = 0;
    for u in s.encode_utf16() {
        if n + 1 >= cap {
            break;
        }
        unsafe { *dst.add(n) = u };
        n += 1;
    }
    unsafe { *dst.add(n) = 0 };
}

pub fn scale(v: i32, dpi: u32) -> i32 {
    (v as i64 * dpi as i64 / 96) as i32
}

pub fn unscale(v: i32, dpi: u32) -> i32 {
    (v as i64 * 96 / dpi.max(1) as i64) as i32
}

pub fn loword(v: usize) -> u16 {
    (v & 0xffff) as u16
}

pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xffff) as u16
}

/// Signed x and y from a mouse message's `lParam`.
pub fn point_from_lparam(l: LPARAM) -> (i32, i32) {
    (loword(l as usize) as i16 as i32, hiword(l as usize) as i16 as i32)
}

/// # Safety
/// Plain `CreateWindowExW` for a child control; `class` must be a valid class name.
pub unsafe fn child(
    parent: HWND,
    class: *const u16,
    text: &str,
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    id: usize,
) -> HWND {
    let text = wide(text);
    unsafe {
        CreateWindowExW(
            ex_style,
            class,
            text.as_ptr(),
            style,
            0,
            0,
            0,
            0,
            parent,
            id as HMENU,
            GetModuleHandleW(null()),
            null(),
        )
    }
}

/// # Safety
/// `hwnd` must be a window.
pub unsafe fn send(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    unsafe { SendMessageW(hwnd, msg, w, l) }
}

/// # Safety
/// `hwnd` must be a window.
pub unsafe fn window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        let mut buf = vec![0u16; len as usize + 1];
        GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        from_wide(&buf)
    }
}

pub fn exe_path() -> PathBuf {
    let mut buf = vec![0u16; 32_768];
    let n = unsafe { GetModuleFileNameW(null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    PathBuf::from(String::from_utf16_lossy(&buf[..n]))
}
