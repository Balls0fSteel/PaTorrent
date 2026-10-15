//! Phase 0 window: engine wiring, DPI-aware font, status bar. Replaced by the real main window in
//! Phase 3; the plumbing (alert notify → `WM_APP`, 1 Hz refresh timer, DPI handling) carries over.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use libretorrent_core::{
    Alert, MockConfig, Session, SessionHandle, SessionStats, Settings, TorrentId, TorrentStatus,
};
use libretorrent_frontend::fmt;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_WINDOW, CreateFontIndirectW, DT_LEFT, DT_NOPREFIX, DT_WORDBREAK, DeleteObject,
    DrawTextW, EndPaint, HBRUSH, HFONT, InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    ICC_BAR_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, SB_SETPARTS, SB_SETTEXTW, SBARS_SIZEGRIP,
    STATUSCLASSNAMEW,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect,
    GetMessageW, IDC_ARROW, KillTimer, LoadCursorW, MSG, NONCLIENTMETRICSW, PostMessageW, PostQuitMessage,
    RegisterClassExW, SPI_GETNONCLIENTMETRICS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SendMessageW,
    SetTimer, SetWindowPos, TranslateMessage, WM_APP, WM_CREATE, WM_DESTROY, WM_DPICHANGED, WM_PAINT,
    WM_SETFONT, WM_SIZE, WM_TIMER, WNDCLASSEXW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows_sys::core::w;

/// Posted by the engine thread when alerts are waiting.
const WM_APP_ALERTS: u32 = WM_APP + 1;
const TIMER_REFRESH: usize = 1;
const LOG_LINES: usize = 10;

struct App {
    session: Option<Session>,
    handle: SessionHandle,
    rows: HashMap<TorrentId, TorrentStatus>,
    stats: SessionStats,
    log: Vec<String>,
    alerts: Vec<Alert>,
    font: HFONT,
    status_bar: HWND,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn run() {
    unsafe {
        let icc =
            INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_BAR_CLASSES };
        InitCommonControlsEx(&icc);
        let instance = GetModuleHandleW(null());
        let class = w!("LibreTorrentMainWnd");
        let mut wc: WNDCLASSEXW = zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.style = CS_HREDRAW | CS_VREDRAW;
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = instance;
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.hbrBackground = (COLOR_WINDOW + 1) as usize as HBRUSH;
        wc.lpszClassName = class;
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class,
            w!("LibreTorrent"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                on_create(hwnd);
                0
            }
            WM_TIMER if wparam == TIMER_REFRESH => {
                with_app(|app| {
                    app.handle.post_torrent_updates();
                    app.handle.post_session_stats();
                });
                0
            }
            WM_APP_ALERTS => {
                on_alerts(hwnd);
                0
            }
            WM_SIZE => {
                layout(hwnd);
                0
            }
            WM_DPICHANGED => {
                let r = &*(lparam as *const RECT);
                set_font(hwnd, GetDpiForWindow(hwnd));
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                0
            }
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_DESTROY => {
                KillTimer(hwnd, TIMER_REFRESH);
                // Dropping the session shuts the engine down; take it out of the RefCell first so
                // no borrow is held while that blocks.
                let app = APP.with(|a| a.borrow_mut().take());
                if let Some(mut app) = app {
                    app.handle.clear_alert_notify();
                    if let Some(s) = app.session.take() {
                        s.shutdown();
                    }
                    DeleteObject(app.font);
                }
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

unsafe fn on_create(hwnd: HWND) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        let scale = |v: i32| v * dpi as i32 / 96;
        SetWindowPos(
            hwnd,
            null_mut(),
            0,
            0,
            scale(820),
            scale(520),
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );

        let status_bar = CreateWindowExW(
            0,
            STATUSCLASSNAMEW,
            null(),
            WS_CHILD | WS_VISIBLE | SBARS_SIZEGRIP,
            0,
            0,
            0,
            0,
            hwnd,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );

        let torrents = demo_torrents();
        let session = Session::start_mock(
            Settings::default(),
            MockConfig { initial_torrents: torrents, ..MockConfig::default() },
        );
        let handle = session.handle();
        // Runs on the engine thread: only post a message. HWND is not Send, so pass its bits.
        let target = hwnd as isize;
        handle.set_alert_notify(move || {
            PostMessageW(target as HWND, WM_APP_ALERTS, 0, 0);
        });
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                session: Some(session),
                handle,
                rows: HashMap::new(),
                stats: SessionStats::default(),
                log: Vec::new(),
                alerts: Vec::new(),
                font: null_mut(),
                status_bar,
            })
        });
        set_font(hwnd, dpi);
        SetTimer(hwnd, TIMER_REFRESH, 1000, None);
        PostMessageW(hwnd, WM_TIMER, TIMER_REFRESH, 0);
    }
}

/// `--demo N` loads N synthetic torrents into the simulated engine.
fn demo_torrents() -> usize {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == "--demo")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

unsafe fn set_font(hwnd: HWND, dpi: u32) {
    unsafe {
        let mut ncm: NONCLIENTMETRICSW = zeroed();
        ncm.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            &mut ncm as *mut _ as *mut c_void,
            0,
            dpi,
        );
        let font = CreateFontIndirectW(&ncm.lfMessageFont);
        let Some((old, status_bar)) =
            with_app(|app| (std::mem::replace(&mut app.font, font), app.status_bar))
        else {
            DeleteObject(font);
            return;
        };
        SendMessageW(status_bar, WM_SETFONT, font as WPARAM, 1);
        if !old.is_null() {
            DeleteObject(old);
        }
        layout(hwnd);
    }
}

unsafe fn layout(hwnd: HWND) {
    unsafe {
        let Some(status_bar) = with_app(|app| app.status_bar) else { return };
        SendMessageW(status_bar, WM_SIZE, 0, 0);
        let mut rc: RECT = zeroed();
        GetClientRect(hwnd, &mut rc);
        let dpi = GetDpiForWindow(hwnd) as i32;
        let w = rc.right;
        let parts = [w - 380 * dpi / 96, w - 190 * dpi / 96, -1];
        SendMessageW(status_bar, SB_SETPARTS, parts.len(), parts.as_ptr() as LPARAM);
        InvalidateRect(hwnd, null(), 1);
    }
}

unsafe fn on_alerts(hwnd: HWND) {
    let texts = with_app(|app| {
        let mut alerts = std::mem::take(&mut app.alerts);
        app.handle.pop_alerts(&mut alerts);
        for a in alerts.drain(..) {
            match a {
                Alert::StateUpdate(v) => {
                    for st in v {
                        app.rows.insert(st.id, st);
                    }
                }
                Alert::SessionStats(s) => app.stats = s,
                Alert::TorrentRemoved { id, .. } => {
                    app.rows.remove(&id);
                }
                other => {
                    if app.log.len() == LOG_LINES {
                        app.log.remove(0);
                    }
                    app.log.push(other.message());
                }
            }
        }
        app.alerts = alerts;
        let s = &app.stats;
        (
            app.status_bar,
            [
                format!("DHT: {} nodes", s.dht_nodes),
                format!("D: {} T: {}", fmt::speed_always(s.download_rate), fmt::size(s.recv_payload_bytes)),
                format!("U: {} T: {}", fmt::speed_always(s.upload_rate), fmt::size(s.sent_payload_bytes)),
            ],
        )
    });
    let Some((status_bar, parts)) = texts else { return };
    unsafe {
        for (i, text) in parts.iter().enumerate() {
            let w = wide(text);
            SendMessageW(status_bar, SB_SETTEXTW, i, w.as_ptr() as LPARAM);
        }
        InvalidateRect(hwnd, null(), 1);
    }
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let body = with_app(|app| {
            let downloading = app.rows.values().filter(|st| !st.is_finished).count();
            let mut s = format!(
                "LibreTorrent {} (Phase 0 skeleton, simulated engine)\n\n\
                 {} torrents loaded, {} downloading, {} complete.\n\
                 Start with --demo 10000 to load synthetic torrents.\n\nRecent events:\n",
                libretorrent_core::VERSION,
                app.rows.len(),
                downloading,
                app.rows.len() - downloading,
            );
            for line in &app.log {
                s.push_str("    ");
                s.push_str(line);
                s.push('\n');
            }
            (s, app.font)
        });
        if let Some((text, font)) = body {
            let mut rc: RECT = zeroed();
            GetClientRect(hwnd, &mut rc);
            let pad = 12 * GetDpiForWindow(hwnd) as i32 / 96;
            rc.left += pad;
            rc.top += pad;
            let old = SelectObject(hdc, font);
            SetBkMode(hdc, TRANSPARENT as i32);
            let w = wide(&text);
            DrawTextW(hdc, w.as_ptr(), -1, &mut rc, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX);
            SelectObject(hdc, old);
        }
        EndPaint(hwnd, &ps);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
