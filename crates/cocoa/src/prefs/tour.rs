//! A test aid for the VM (as `snapshots`): with `ROOKPARCEL_SNAPSHOTS` naming a folder that
//! holds a file `prefs-tour` (a test script makes it; `run.sh` passes no other variable),
//! opening Preferences starts a tour that shows every page and then drives the controls through
//! their own actions (`performClick:`, a text field's action), the way a click or Return would: a check box, a bad port, a good one, a bad one again, the
//! Advanced filter, a drag on the Scheduler's grid, closing the window with the bad port, and
//! opening it again at the port. After each step it keeps the snapshot of the moment as
//! `<step>-window-<n>.png` and `<step>.txt`, and logs the `rookparcel.ini` lines the step is
//! about. Accessibility cannot reach the windows while the VM's screen is locked; this can.

use std::cell::Cell;
use std::path::PathBuf;

use super::{Ctl, state};
use crate::app::peek;
use crate::canvas::Mouse;

thread_local! {
    static STEP: Cell<Option<usize>> = const { Cell::new(None) };
}

fn dir() -> Option<PathBuf> {
    let dir = std::env::var_os("ROOKPARCEL_SNAPSHOTS").filter(|d| !d.is_empty()).map(PathBuf::from)?;
    dir.join("prefs-tour").exists().then_some(dir)
}

/// Runs the tour once, when it is asked for: every step at once (a background app's timers
/// are held back by App Nap on the locked VM, so nothing waits on one).
pub(super) fn start() {
    if dir().is_none() || STEP.with(Cell::get).is_some() {
        return;
    }
    STEP.with(|s| s.set(Some(0)));
    eprintln!("prefs tour: start");
    while step() {}
}
/// Keeps the snapshot of now under `name`.
fn keep(name: &str) {
    let Some(dir) = dir() else { return };
    if let Some(content) = state().and_then(|st| st.window.contentView()) {
        content.layoutSubtreeIfNeeded();
    }
    crate::snapshots::take();
    for n in 1..=4 {
        let from = dir.join(format!("window-{n}.png"));
        if from.exists() {
            let _ = std::fs::copy(&from, dir.join(format!("{name}-window-{n}.png")));
        }
    }
    let _ = std::fs::copy(dir.join("windows.txt"), dir.join(format!("{name}.txt")));
    eprintln!("prefs tour: kept {name}");
}

/// Logs the lines of `rookparcel.ini` holding any of `words`.
fn ini(words: &[&str]) {
    let Some(file) = peek(|c| c.dirs().config.join("rookparcel.ini")) else { return };
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| words.iter().any(|w| l.contains(w))).collect();
    eprintln!("prefs tour: ini {words:?}: {lines:?}");
}

/// Types `text` into the field `key` and sends its action, as Return does.
fn enter(key: &str, text: &str) {
    let Some(st) = state() else { return };
    let Some(f) = st.fields.get(key) else { return };
    if let Ctl::Text(t) = &f.ctl {
        f.ctl.set(text);
        // SAFETY: the field's own action and target (`RPTarget`'s `perform:`).
        unsafe { t.sendAction_to(t.action(), t.target().as_deref()) };
    }
    eprintln!("prefs tour: {key} = {text}");
}

fn click(key: &str) {
    let Some(st) = state() else { return };
    if let Some(Ctl::Check(b)) = st.fields.get(key).map(|f| &f.ctl) {
        // SAFETY: a nil sender, as a click has none to pass.
        unsafe { b.performClick(None) };
        eprintln!("prefs tour: clicked {key}");
    }
}

fn page(key: &str) {
    let Some(st) = state() else { return };
    if let Some(i) = st.pages.iter().position(|p| p.key == key) {
        super::show_page_selected(&st, i);
    }
}

/// One step; false at the end.
fn step() -> bool {
    let Some(st) = state() else { return false };
    let n = STEP.with(|s| s.get()).unwrap_or(0);
    STEP.with(|s| s.set(Some(n + 1)));
    let pages = st.pages.len();
    if n < pages {
        if n > 0 {
            keep(&format!("page-{:02}", n));
        }
        super::show_page_selected(&st, n);
        return true;
    }
    match n - pages {
        0 => {
            keep(&format!("page-{pages:02}"));
            page("bittorrent");
            ini(&["enable_dht"]);
            click("enable_dht");
        }
        1 => {
            keep("dht-off");
            ini(&["enable_dht"]);
            page("connection");
            enter("port", "99999");
        }
        2 => {
            keep("port-bad");
            ini(&["listen"]);
            enter("port", "51413");
        }
        3 => {
            keep("port-good");
            ini(&["listen"]);
            enter("port", "0");
        }
        4 => {
            keep("port-bad2");
            page("advanced");
            if let Some(a) = &st.advanced {
                a.filter.setStringValue(&objc2_foundation::NSString::from_str("dht"));
                super::advanced::filter_changed(&st);
            }
        }
        5 => {
            keep("advanced-dht");
            page("scheduler");
            click(rookparcel_frontend::pref_pages::SCHED_ENABLE);
        }
        6 => {
            // a drag over Monday's first hours, as the mouse would
            if let Some(c) = &st.sched {
                let b = c.bounds();
                let (w, h) = (b.size.width, b.size.height);
                let (x0, y) = (w * 0.25, h * 0.2);
                for m in [Mouse::Down(x0, y), Mouse::Drag(x0 + w * 0.2, y), Mouse::Up(x0 + w * 0.2, y)] {
                    super::sched_grid::mouse(c, m);
                }
                super::sched_grid::mouse(c, Mouse::Moved(x0 + w * 0.1, y));
            }
        }
        7 => {
            keep("scheduler-painted");
            ini(&["sched"]);
            page("connection");
            enter("port", "99999");
        }
        8 => {
            keep("port-bad3");
            st.window.performClose(None);
        }
        9 => {
            ini(&["listen"]);
            super::open_at("port");
        }
        10 => {
            keep("reopened");
            eprintln!("prefs tour: done");
            return false;
        }
        _ => return false,
    }
    true
}
