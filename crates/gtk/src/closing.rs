//! The window shown while PaTorrent closes (D47), as in the Windows GUI: the engine stops on its
//! own thread ([`Session::begin_shutdown`](patorrent_core::Session::begin_shutdown)); when one
//! step of that takes more than a second (never the trackers' "stopped" announces,
//! [`ShowTimer`]), or at once when the desktop session is ending, this window says
//! what it is waiting for, lists the moves still to finish (the list scrolls) with their
//! progress and destination, and for how long. Quit Now lists what quitting early gives up and
//! asks; it is not offered while the torrent list and progress are being saved. The texts and
//! rules are `patorrent_frontend::closing`'s.
//!
//! The application is held until the engine is done, so it outlives the main window; logout
//! and suspend are inhibited meanwhile (the desktop's session manager, or the portal, shows the
//! reason).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use patorrent_core::{Exit, ExitStatus};
use patorrent_frontend::closing::{self, ShowTimer};
use patorrent_frontend::lang::{gtk_mnemonic, plain};
use patorrent_frontend::tr;

/// The window's width; its height follows what it shows.
const WIDTH: i32 = 620;

struct Shown {
    window: gtk::Window,
    phase: gtk::Label,
    scroller: gtk::ScrolledWindow,
    grid: gtk::Grid,
    bar: gtk::ProgressBar,
    elapsed: gtk::Label,
    quit: gtk::Button,
}

struct State {
    app: gtk::Application,
    exit: Option<Exit>,
    start: Instant,
    timer: ShowTimer,
    /// The session is ending: show at once.
    logout: bool,
    shown: Option<Shown>,
    /// What the window shows, to set only what changed.
    last: Option<(ExitStatus, u64)>,
    rows: Vec<[String; 3]>,
    /// Keeps the application running without a window.
    _hold: gio::ApplicationHoldGuard,
    inhibit: u32,
}

/// Follows `exit` until the engine is done (or Quit Now got it past saving), behind the window
/// when it takes long, then lets the application end. `logout`: the desktop session is ending.
pub fn start(app: &gtk::Application, exit: Exit, logout: bool) {
    let inhibit = app.inhibit(
        None::<&gtk::Window>,
        gtk::ApplicationInhibitFlags::LOGOUT | gtk::ApplicationInhibitFlags::SUSPEND,
        Some(tr!(EndSessionSaving)),
    );
    let start = Instant::now();
    let st = Rc::new(RefCell::new(State {
        app: app.clone(),
        exit: Some(exit),
        start,
        timer: ShowTimer::new(start),
        logout,
        shown: None,
        last: None,
        rows: Vec::new(),
        _hold: app.hold(),
        inhibit,
    }));
    if tick(&st) == glib::ControlFlow::Continue {
        glib::timeout_add_local(closing::REFRESH, move || tick(&st));
    }
}

fn tick(st: &Rc<RefCell<State>>) -> glib::ControlFlow {
    let s = {
        let state = st.borrow();
        let Some(exit) = &state.exit else { return glib::ControlFlow::Break };
        let s = exit.status();
        if !closing::done(exit, &s) { Some(s) } else { None }
    };
    let Some(s) = s else {
        finish(st);
        return glib::ControlFlow::Break;
    };
    let show = {
        let mut state = st.borrow_mut();
        let due = state.timer.due(&s, Instant::now());
        state.shown.is_none() && (state.logout || due)
    };
    if show {
        build(st);
    }
    refresh(st, s);
    glib::ControlFlow::Continue
}

/// The engine is done: the window goes, the session may end, the application with it.
fn finish(st: &Rc<RefCell<State>>) {
    let mut state = st.borrow_mut();
    if let Some(exit) = state.exit.take() {
        if exit.is_finished() { exit.wait() } else { exit.abandon() }
    }
    if state.inhibit != 0 {
        state.app.uninhibit(state.inhibit);
        state.inhibit = 0;
    }
    if let Some(shown) = state.shown.take() {
        shown.window.destroy();
    }
    // dropping the state releases the hold
}

fn build(st: &Rc<RefCell<State>>) {
    let app = st.borrow().app.clone();
    // as tall as what it shows (`refresh` shrinks it again when the list goes)
    let window = gtk::Window::builder().application(&app).title(tr!(ExitTitle)).default_width(WIDTH).build();
    let phase = gtk::Label::new(None);
    phase.set_xalign(0.0);
    phase.set_wrap(true);
    let grid = gtk::Grid::builder().column_spacing(18).row_spacing(4).build();
    let scroller = gtk::ScrolledWindow::builder()
        .child(&grid)
        .vexpand(true)
        .visible(false)
        .min_content_height(160)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .has_frame(true)
        .build();
    grid.set_margin_start(6);
    grid.set_margin_end(6);
    grid.set_margin_top(4);
    grid.set_margin_bottom(4);
    let bar = gtk::ProgressBar::new();
    let elapsed = gtk::Label::new(None);
    elapsed.set_xalign(0.0);
    elapsed.set_hexpand(true);
    let quit = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(ExitQuitNow)));
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bottom.append(&elapsed);
    bottom.append(&quit);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(14);
    body.set_margin_bottom(12);
    body.set_margin_start(14);
    body.set_margin_end(14);
    body.append(&phase);
    body.append(&scroller);
    body.append(&bar);
    body.append(&bottom);
    window.set_child(Some(&body));
    let weak = Rc::downgrade(st);
    quit.connect_clicked(move |_| {
        if let Some(st) = weak.upgrade() {
            quit_now(&st);
        }
    });
    // the title bar's close means Quit Now, with its question
    let weak = Rc::downgrade(st);
    window.connect_close_request(move |_| {
        if let Some(st) = weak.upgrade() {
            quit_now(&st);
        }
        glib::Propagation::Stop
    });
    window.present();
    st.borrow_mut().shown = Some(Shown { window, phase, scroller, grid, bar, elapsed, quit });
}

/// Shows what the engine is doing now.
fn refresh(st: &Rc<RefCell<State>>, s: ExitStatus) {
    let mut state = st.borrow_mut();
    let secs = state.start.elapsed().as_secs();
    let State { shown, last, rows: shown_rows, .. } = &mut *state;
    let Some(w) = shown else { return };
    let fraction = closing::fraction(&s);
    if fraction.is_none() {
        // runs back and forth for steps it cannot measure
        w.bar.pulse();
    }
    if last.as_ref().is_some_and(|(l, t)| *l == s && *t == secs) {
        return;
    }
    w.phase.set_text(&closing::phase_text(&s));
    w.elapsed.set_text(&closing::elapsed_text(secs));
    if let Some(f) = fraction {
        w.bar.set_fraction(f);
    }
    let rows = closing::rows(&s);
    if rows != *shown_rows {
        fill(&w.grid, &rows);
        *shown_rows = rows;
    }
    let list = !shown_rows.is_empty();
    if w.scroller.is_visible() != list {
        w.scroller.set_visible(list);
        // back to the height the content needs: a window keeps its size when content goes
        w.window.set_default_size(WIDTH, -1);
    }
    w.quit.set_sensitive(closing::quit_offered(&s));
    *last = Some((s, secs));
}

/// The list: a title row, then one row per move. Names are cut at the end and folders in the
/// middle; the whole text is the tooltip.
fn fill(grid: &gtk::Grid, rows: &[[String; 3]]) {
    while let Some(c) = grid.first_child() {
        grid.remove(&c);
    }
    let cell = |text: &str, col: i32, row: i32, bold: bool| {
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        if bold {
            l.add_css_class("heading");
        }
        match col {
            0 => {
                l.set_ellipsize(pango::EllipsizeMode::End);
                l.set_hexpand(true);
                l.set_width_chars(18);
            }
            2 => {
                l.set_ellipsize(pango::EllipsizeMode::Middle);
                l.set_hexpand(true);
                l.set_width_chars(18);
            }
            _ => {}
        }
        if !bold && col != 1 {
            l.set_tooltip_text(Some(text));
        }
        grid.attach(&l, col, row, 1, 1);
    };
    for (col, title) in closing::columns().iter().enumerate() {
        cell(title, col as i32, 0, true);
    }
    for (i, r) in rows.iter().enumerate() {
        for (col, text) in r.iter().enumerate() {
            cell(text, col as i32, i as i32 + 1, false);
        }
    }
}

/// Quit Now: says what it gives up and asks, then cuts the waits short (the window ends once
/// the engine is past saving).
fn quit_now(st: &Rc<RefCell<State>>) {
    let (s, window) = {
        let state = st.borrow();
        let Some(exit) = &state.exit else { return };
        (exit.status(), state.shown.as_ref().map(|w| w.window.clone()))
    };
    if !closing::quit_offered(&s) {
        return;
    }
    let Some(q) = closing::question(&closing::consequences(&s)) else {
        hurry(st);
        return;
    };
    let d = gtk::AlertDialog::builder()
        .message(tr!(ExitTitle))
        .detail(q)
        .buttons([plain(tr!(BtnCancel)), plain(tr!(ExitQuitNow))])
        .cancel_button(0)
        .default_button(0)
        .modal(true)
        .build();
    let weak = Rc::downgrade(st);
    d.choose(window.as_ref(), None::<&gio::Cancellable>, move |r| {
        if let (Ok(1), Some(st)) = (r, weak.upgrade()) {
            hurry(&st);
        }
    });
}

fn hurry(st: &Rc<RefCell<State>>) {
    if let Some(exit) = &st.borrow().exit {
        exit.hurry();
    }
}
