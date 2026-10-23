//! `PATORRENT_TIMING=1` and `PATORRENT_BENCH=1`: the torrent list's latency at scale (Road to
//! 1.0 item 5, D21), measured on the GUI thread itself.
//!
//! With `PATORRENT_TIMING=1` the list's hot paths (the alert batch, the list refresh, the tree
//! counts, the timer) log their wall time and the GUI thread's CPU to stderr. `PATORRENT_BENCH=1`
//! runs a script once the list has stopped growing, through the messages a user's clicks send:
//! sorts by Name, Size, Status and Down Speed and their reversals (`LVN_COLUMNCLICK`), the
//! status updates with a sort active, page scrolls and jumps (`WM_VSCROLL`,
//! `LVM_ENSUREVISIBLE`), Select All, category switches in the tree, adding a magnet, removing
//! it and removing 1,000 rows, then 60 s of idle; each step is timed until its rows are painted
//! (`UpdateWindow`). Then the window closes. The script removes torrents: run it on a copy of a
//! state folder, with `ui.confirm_delete=0` and `ui.show_add_dialog=0` in `patorrent.ini`.
//! Unset, both cost a load of an initialised `OnceLock`.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{FILETIME, HWND, LPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetProcessTimes, GetThreadTimes,
};
use windows_sys::Win32::UI::Controls::{
    LVM_ENSUREVISIBLE, LVM_GETITEMCOUNT, LVN_COLUMNCLICK, NMHDR, NMLISTVIEW, TVGN_CARET, TVM_SELECTITEM,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    KillTimer, PostMessageW, SB_BOTTOM, SB_PAGEDOWN, SB_TOP, SetTimer, WM_COMMAND, WM_NOTIFY, WM_VSCROLL,
};

use super::{IDC_LIST, cmd, with_app};
use crate::controls;
use crate::util::send;
use patorrent_frontend::columns::Column;

/// The bench script's timer on the main window.
pub(super) const TIMER_BENCH: usize = 2;

struct Mode {
    timing: bool,
    bench: bool,
    /// The thread cycle counter's rate, measured once.
    cycles_per_ms: f64,
}

fn mode() -> &'static Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    MODE.get_or_init(|| {
        let set = |k| std::env::var_os(k).is_some_and(|v| !v.is_empty() && v != "0");
        let bench = set("PATORRENT_BENCH");
        let timing = bench || set("PATORRENT_TIMING");
        Mode { timing, bench, cycles_per_ms: if timing { calibrate() } else { 1.0 } }
    })
}

pub(super) fn timing() -> bool {
    mode().timing
}

// In windows-sys behind `Win32_System_WindowsProgramming`, a feature for one function.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryThreadCycleTime(thread: windows_sys::Win32::Foundation::HANDLE, cycles: *mut u64) -> i32;
}

fn cycles() -> u64 {
    let mut c = 0u64;
    // SAFETY: the pseudo handle of the current thread, a valid out pointer.
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) };
    c
}

/// Spins 100 ms to relate thread cycles to milliseconds.
fn calibrate() -> f64 {
    let (t0, c0) = (Instant::now(), cycles());
    while t0.elapsed() < Duration::from_millis(100) {
        std::hint::spin_loop();
    }
    (cycles() - c0) as f64 / (t0.elapsed().as_secs_f64() * 1000.0)
}

fn cpu_ms(c0: u64) -> f64 {
    (cycles() - c0) as f64 / mode().cycles_per_ms
}

#[derive(Clone, Copy, Default)]
struct Stat {
    n: u32,
    wall: f64,
    wall_max: f64,
    cpu: f64,
}

thread_local! {
    static STATS: RefCell<Vec<(&'static str, Stat)>> = const { RefCell::new(Vec::new()) };
    static BENCH: RefCell<Option<Bench>> = const { RefCell::new(None) };
}

/// Times a stretch of the GUI thread until dropped (`None` unless timing is on).
pub(super) struct Span {
    label: &'static str,
    t0: Instant,
    c0: u64,
}

pub(super) fn span(label: &'static str) -> Option<Span> {
    timing().then(|| Span { label, t0: Instant::now(), c0: cycles() })
}

impl Drop for Span {
    fn drop(&mut self) {
        let wall = self.t0.elapsed().as_secs_f64() * 1000.0;
        let cpu = cpu_ms(self.c0);
        STATS.with(|s| {
            let mut s = s.borrow_mut();
            let i = match s.iter().position(|(l, _)| *l == self.label) {
                Some(i) => i,
                None => {
                    s.push((self.label, Stat::default()));
                    s.len() - 1
                }
            };
            let st = &mut s[i].1;
            st.n += 1;
            st.wall += wall;
            st.wall_max = st.wall_max.max(wall);
            st.cpu += cpu;
        });
        if !mode().bench {
            eprintln!("timing {}: {wall:.2} ms wall, {cpu:.2} ms cpu", self.label);
        }
    }
}

fn take_stats() -> Vec<(&'static str, Stat)> {
    STATS.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

enum Wait {
    /// The list is still filling: its count and since when it has not changed.
    Rows(usize, Instant),
    /// An add or remove: the row count to reach, with the clock started.
    Count {
        label: &'static str,
        target: usize,
        t0: Instant,
        c0: u64,
    },
    /// An idle stretch; its label when the status updates' stats are reported after it.
    Until(Instant, Option<&'static str>),
    Next,
}

struct Bench {
    start: Instant,
    step: usize,
    wait: Wait,
    /// Rows when the list settled.
    rows: usize,
    /// The added magnet's display name.
    added: String,
}

/// Starts the script if `PATORRENT_BENCH` is set (from the main window's creation).
pub(super) fn start(hwnd: HWND) {
    if !mode().bench {
        return;
    }
    BENCH.with(|b| {
        *b.borrow_mut() = Some(Bench {
            start: Instant::now(),
            step: 0,
            wait: Wait::Rows(0, Instant::now()),
            rows: 0,
            added: format!("patorrent-bench-{}", std::process::id()),
        })
    });
    // SAFETY: the main window is live.
    unsafe { SetTimer(hwnd, TIMER_BENCH, 100, None) };
    eprintln!("bench | step | wall ms | gui thread cpu ms");
}

fn lv() -> HWND {
    with_app(|a| a.lv).unwrap_or(std::ptr::null_mut())
}

fn count() -> usize {
    // SAFETY: a live list view (or null, which answers 0).
    unsafe { send(lv(), LVM_GETITEMCOUNT, 0, 0) as usize }
}

/// Paints what the step invalidated.
fn paint() {
    // SAFETY: a live window.
    unsafe { UpdateWindow(lv()) };
}

fn report(label: &str, t0: Instant, c0: u64) {
    let wall = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("bench | {label} | {wall:.2} | {:.2}", cpu_ms(c0));
}

/// Runs `f` and paints, timed.
fn measure(label: &str, f: impl FnOnce()) {
    let (t0, c0) = (Instant::now(), cycles());
    f();
    paint();
    report(label, t0, c0);
}

fn click(hwnd: HWND, col: Column) -> bool {
    let Some(i) = with_app(|a| a.shown.iter().position(|&c| c == col)).flatten() else {
        eprintln!("bench | {col:?} column hidden, skipped | - | -");
        return false;
    };
    // SAFETY: zeroed is a valid NMLISTVIEW.
    let mut nm: NMLISTVIEW = unsafe { std::mem::zeroed() };
    nm.hdr = NMHDR { hwndFrom: lv(), idFrom: IDC_LIST, code: LVN_COLUMNCLICK };
    nm.iItem = -1;
    nm.iSubItem = i as i32;
    // SAFETY: the notification is handled synchronously while `nm` lives.
    unsafe { send(hwnd, WM_NOTIFY, IDC_LIST, &nm as *const _ as LPARAM) };
    true
}

fn report_stats(label: &str) {
    for (l, s) in take_stats() {
        if s.n == 0 {
            continue;
        }
        eprintln!(
            "bench | {label}: {l} x{} avg / max | {:.2} / {:.2} | {:.2}",
            s.n,
            s.wall / s.n as f64,
            s.wall_max,
            s.cpu / s.n as f64
        );
    }
}

fn filetime(f: FILETIME) -> u64 {
    (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)
}

/// User + kernel time of the GUI thread and of the process, in 100 ns units.
fn cpu_times() -> (u64, u64) {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    let (mut pc, mut pe, mut pk, mut pu) = (z, z, z, z);
    // SAFETY: pseudo handles and valid out pointers.
    unsafe {
        GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
        GetProcessTimes(GetCurrentProcess(), &mut pc, &mut pe, &mut pk, &mut pu);
    }
    (filetime(k) + filetime(u), filetime(pk) + filetime(pu))
}

thread_local! {
    static IDLE: RefCell<Option<(Instant, (u64, u64))>> = const { RefCell::new(None) };
}

/// From `on_alerts`: an add or remove the script waits for may be done.
pub(super) fn after_alerts() {
    if !mode().bench {
        return;
    }
    let Some(mut b) = BENCH.with(|b| b.borrow_mut().take()) else { return };
    if let Wait::Count { label, target, t0, c0 } = b.wait {
        if count() == target {
            paint();
            report(label, t0, c0);
            b.wait = Wait::Next;
        }
    }
    BENCH.with(|cell| *cell.borrow_mut() = Some(b));
}

/// `TIMER_BENCH`: advances the script.
pub(super) fn on_timer(hwnd: HWND) {
    let Some(mut b) = BENCH.with(|b| b.borrow_mut().take()) else { return };
    loop {
        match b.wait {
            Wait::Rows(last, since) => {
                let n = count();
                if n != last || n == 0 {
                    b.wait = Wait::Rows(n, Instant::now());
                    break;
                }
                if since.elapsed() < Duration::from_secs(3) {
                    break;
                }
                b.rows = n;
                eprintln!(
                    "bench | {n} rows, settled at {:.2} s (3 s before this line) | - | -",
                    b.start.elapsed().as_secs_f64() - 3.0
                );
                take_stats();
                b.wait = Wait::Next;
            }
            Wait::Count { .. } => break,
            Wait::Until(t, label) => {
                if Instant::now() < t {
                    break;
                }
                if let Some(l) = label {
                    report_stats(l);
                }
                b.wait = Wait::Next;
            }
            Wait::Next => {
                let step = b.step;
                b.step += 1;
                match run_step(hwnd, step, &mut b) {
                    Some(w) => b.wait = w,
                    None => {
                        // SAFETY: the main window is live.
                        unsafe {
                            KillTimer(hwnd, TIMER_BENCH);
                            PostMessageW(hwnd, WM_COMMAND, cmd::EXIT as usize, 0);
                        }
                        return;
                    }
                }
            }
        }
    }
    BENCH.with(|cell| *cell.borrow_mut() = Some(b));
}

fn idle(secs: u64, label: &'static str) -> Wait {
    take_stats();
    Wait::Until(Instant::now() + Duration::from_secs(secs), Some(label))
}

/// One step of the script; `None` when it is over.
fn run_step(hwnd: HWND, step: usize, b: &mut Bench) -> Option<Wait> {
    const SORTS: [(Column, &str); 4] = [
        (Column::Name, "Name"),
        (Column::Size, "Size"),
        (Column::Status, "Status"),
        (Column::DownSpeed, "Down Speed"),
    ];
    // the 60 s idle (28, 29) runs right after the first paint
    let step = match step {
        0 => 0,
        1 => 28,
        2 => 29,
        n @ 3..=29 => n - 2,
        _ => 30,
    };
    let lv = lv();
    // SAFETY (all the sends below): live windows, synchronous messages.
    unsafe {
        match step {
            0 => measure("paint the first page", || {
                InvalidateRect(lv, std::ptr::null(), 0);
            }),
            1..=8 => {
                let (col, name) = SORTS[(step - 1) / 2];
                let what = if step % 2 == 1 { "sort by" } else { "reverse" };
                let (t0, c0) = (Instant::now(), cycles());
                if click(hwnd, col) {
                    paint();
                    report(&format!("{what} {name}"), t0, c0);
                }
            }
            9 => return Some(idle(10, "status updates, sorted by Down Speed")),
            10 => {
                click(hwnd, Column::Name);
                paint();
                return Some(idle(10, "status updates, sorted by Name"));
            }
            11 => {
                let (t0, c0) = (Instant::now(), cycles());
                let mut worst = 0f64;
                for _ in 0..20 {
                    let t = Instant::now();
                    send(lv, WM_VSCROLL, SB_PAGEDOWN as usize, 0);
                    paint();
                    worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
                }
                let wall = t0.elapsed().as_secs_f64() * 1000.0 / 20.0;
                eprintln!(
                    "bench | page down (avg of 20, max {worst:.2}) | {wall:.2} | {:.2}",
                    cpu_ms(c0) / 20.0
                );
            }
            12 => measure("scroll to the end (SB_BOTTOM)", || {
                send(lv, WM_VSCROLL, SB_BOTTOM as usize, 0);
            }),
            13 => measure("scroll to the top (SB_TOP)", || {
                send(lv, WM_VSCROLL, SB_TOP as usize, 0);
            }),
            14 => measure("LVM_ENSUREVISIBLE the last row", || {
                send(lv, LVM_ENSUREVISIBLE, count().saturating_sub(1), 0);
            }),
            15 => measure("LVM_ENSUREVISIBLE the middle row", || {
                send(lv, LVM_ENSUREVISIBLE, count() / 2, 0);
            }),
            16 => measure("Select All (Ctrl+A)", || {
                send(hwnd, WM_COMMAND, cmd::SELECT_ALL as usize, 0);
            }),
            17 => return Some(idle(5, "status updates, all rows selected")),
            18 => measure("clear the selection", || controls::set_state(lv, None, false, false)),
            19..=22 => {
                // All (0), Downloading (1), Completed (2), No label (6), then All again
                let (idx, name) =
                    [(2, "Completed"), (1, "Downloading"), (6, "No Label"), (0, "All")][step - 19];
                let tree_item = with_app(|a| (a.tree, a.tree_items.get(idx).map(|t| t.0)));
                if let Some((tree, Some(item))) = tree_item {
                    measure(&format!("category {name}"), || {
                        send(tree, TVM_SELECTITEM, TVGN_CARET as usize, item);
                    });
                }
            }
            23 => {
                let n = count();
                let hash: String = (0..40)
                    .map(|i| {
                        let x =
                            (std::process::id() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (i % 16 * 4);
                        char::from_digit((x as u32 ^ i as u32) & 0xf, 16).unwrap_or('0')
                    })
                    .collect();
                let uri = format!("magnet:?xt=urn:btih:{hash}&dn={}", b.added);
                let (t0, c0) = (Instant::now(), cycles());
                super::add_magnet(hwnd, &uri);
                return Some(Wait::Count {
                    label: "add a magnet, until its row is painted",
                    target: n + 1,
                    t0,
                    c0,
                });
            }
            24 => return Some(Wait::Until(Instant::now() + Duration::from_secs(2), None)),
            25 => {
                let n = count();
                let name = b.added.clone();
                let row = with_app(|a| {
                    a.list.view().iter().position(|id| a.list.get(*id).is_some_and(|s| s.name == name))
                })
                .flatten();
                let Some(row) = row else {
                    eprintln!("bench | the added torrent is not in the list | - | -");
                    return Some(Wait::Next);
                };
                controls::set_state(lv, None, false, false);
                controls::set_state(lv, Some(row), true, true);
                let (t0, c0) = (Instant::now(), cycles());
                send(hwnd, WM_COMMAND, cmd::REMOVE as usize, 0);
                return Some(Wait::Count {
                    label: "remove one torrent, until painted",
                    target: n - 1,
                    t0,
                    c0,
                });
            }
            26 => return Some(Wait::Until(Instant::now() + Duration::from_secs(2), None)),
            27 => {
                let n = count();
                controls::set_state(lv, None, false, false);
                measure("select 1,000 rows (LVM_SETITEMSTATE each)", || {
                    for i in 0..1000.min(n) {
                        controls::set_state(lv, Some(i), true, false);
                    }
                });
                let (t0, c0) = (Instant::now(), cycles());
                send(hwnd, WM_COMMAND, cmd::REMOVE as usize, 0);
                return Some(Wait::Count {
                    label: "remove 1,000 torrents, until painted",
                    target: n.saturating_sub(1000),
                    t0,
                    c0,
                });
            }
            28 => {
                IDLE.with(|i| *i.borrow_mut() = Some((Instant::now(), cpu_times())));
                eprintln!("bench | idle 60 s from {:.1} s | - | -", b.start.elapsed().as_secs_f64());
                return Some(idle(60, "idle 60 s"));
            }
            29 => {
                if let Some((t0, (th0, p0))) = IDLE.with(|i| i.borrow_mut().take()) {
                    let (th, p) = cpu_times();
                    let secs = t0.elapsed().as_secs_f64();
                    let pct = |d: u64| d as f64 / 1e7 / secs * 100.0;
                    eprintln!(
                        "bench | idle {secs:.1} s: gui thread {:.3} % of a core, process {:.3} % | - | -",
                        pct(th - th0),
                        pct(p - p0)
                    );
                }
            }
            _ => {
                eprintln!(
                    "bench | done at {:.1} s ({} rows at start) | - | -",
                    b.start.elapsed().as_secs_f64(),
                    b.rows
                );
                return None;
            }
        }
    }
    Some(Wait::Next)
}
