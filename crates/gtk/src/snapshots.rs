//! A test aid: with the environment variable `PATORRENT_SNAPSHOTS` naming a folder, every two
//! seconds each window of this process is drawn into `window-<n>.png` there, and `windows.txt`
//! lists each window's title and the text of every label in it, dialogs included. For checks
//! on machines whose screen cannot be captured (a macOS VM without Metal, where `screencapture`
//! shows only the menu bar) or read through accessibility (GTK on macOS exposes no label text).
//! GTK draws the windows itself, so nothing is needed from the desktop.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

const VARIABLE: &str = "PATORRENT_SNAPSHOTS";

/// Starts the snapshots when the variable is set (and not empty).
pub fn start() {
    let Some(dir) = std::env::var_os(VARIABLE).filter(|d| !d.is_empty()).map(PathBuf::from) else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("{VARIABLE}: {}: {e}", dir.display());
        return;
    }
    eprintln!("{VARIABLE}: the windows go to {} every 2 s", dir.display());
    // one soon after the start too
    let first = dir.clone();
    glib::timeout_add_local_once(Duration::from_millis(500), move || take(&first));
    glib::timeout_add_seconds_local(2, move || {
        take(&dir);
        glib::ControlFlow::Continue
    });
}

/// Lists the windows shown now, then draws them (the list first: it is there even where
/// drawing fails); the pictures of windows gone since are removed.
fn take(dir: &Path) {
    let windows: Vec<gtk::Window> = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .filter(|w| w.is_visible())
        .collect();
    let mut list = String::new();
    for (i, window) in windows.iter().enumerate() {
        let title = window.title().map(|t| t.to_string()).unwrap_or_default();
        let _ = writeln!(list, "== window-{}: \"{title}\" {}x{}", i + 1, window.width(), window.height());
        let mut texts = Vec::new();
        label_texts(window.upcast_ref(), &mut texts);
        for t in texts {
            let _ = writeln!(list, "  {}", t.replace('\n', "\n  "));
        }
    }
    let _ = std::fs::write(dir.join("windows.txt"), list);
    for (i, window) in windows.iter().enumerate() {
        if let Err(e) = save_png(window, &dir.join(format!("window-{}.png", i + 1))) {
            eprintln!("{VARIABLE}: window {}: {e}", i + 1);
        }
    }
    // windows that closed since the last round
    for stale in windows.len() + 1.. {
        if std::fs::remove_file(dir.join(format!("window-{stale}.png"))).is_err() {
            break;
        }
    }
}

/// `window` as GTK draws it, without the desktop's decorations.
fn save_png(window: &gtk::Window, path: &Path) -> Result<(), String> {
    let (width, height) = (window.width(), window.height());
    if width <= 0 || height <= 0 {
        return Err("not laid out yet".into());
    }
    let renderer = window.native().and_then(|n| n.renderer()).ok_or("no renderer")?;
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot.to_node().ok_or("nothing drawn")?;
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path).map_err(|e| e.to_string())
}

/// The text of every visible label under `widget`, in the order they are laid out.
fn label_texts(widget: &gtk::Widget, out: &mut Vec<String>) {
    if !widget.is_visible() {
        return;
    }
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        let text = label.text();
        if !text.is_empty() {
            out.push(text.to_string());
        }
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        label_texts(&c, out);
        child = c.next_sibling();
    }
}
