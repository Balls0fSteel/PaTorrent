//! The notification-area icon: tooltip with the transfer rates, completion balloons.

use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::HICON;

const ID: u32 = 1;

fn fill(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for u in s.encode_utf16() {
        if n + 1 >= dst.len() {
            break;
        }
        dst[n] = u;
        n += 1;
    }
    dst[n] = 0;
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut d: NOTIFYICONDATAW = unsafe { zeroed() };
    d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = hwnd;
    d.uID = ID;
    d
}

/// Adds the icon; `callback` is the window message sent for mouse events.
pub fn add(hwnd: HWND, icon: HICON, callback: u32, tip: &str) {
    let mut d = data(hwnd);
    d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    d.uCallbackMessage = callback;
    d.hIcon = icon;
    fill(&mut d.szTip, tip);
    unsafe { Shell_NotifyIconW(NIM_ADD, &d) };
}

pub fn set_tip(hwnd: HWND, tip: &str) {
    let mut d = data(hwnd);
    d.uFlags = NIF_TIP;
    fill(&mut d.szTip, tip);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) };
}

pub fn balloon(hwnd: HWND, title: &str, text: &str) {
    let mut d = data(hwnd);
    d.uFlags = NIF_INFO;
    d.dwInfoFlags = NIIF_INFO;
    fill(&mut d.szInfoTitle, title);
    fill(&mut d.szInfo, text);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) };
}

pub fn remove(hwnd: HWND) {
    let d = data(hwnd);
    unsafe { Shell_NotifyIconW(NIM_DELETE, &d) };
}
