//! A test aid (as GTK's): with `ROOKPARCEL_SNAPSHOTS` naming a folder, every two seconds each
//! visible window of this process is drawn into `window-<n>.png` there (its frame view, through
//! `cacheDisplayInRect:toBitmapImageRep:`, so the toolbar and title bar are in it), and
//! `windows.txt` lists each window's title and size and the text of every visible label, text
//! field, button and table cell in it (`snapshot_text` walks the views). Pictures of windows
//! gone since are removed. For checks over SSH without the screen (docs/MACOS_GUI.md "Test and
//! VM workflow").

use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::PathBuf;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplication, NSBitmapImageFileType, NSButton, NSPopUpButton, NSTabView, NSTextField, NSView, NSWindow,
};
use objc2_foundation::{NSDictionary, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer};

const VARIABLE: &str = "ROOKPARCEL_SNAPSHOTS";

thread_local! {
    static DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Starts the snapshots when the variable is set (and not empty).
pub fn start(mtm: MainThreadMarker) {
    let Some(dir) = std::env::var_os(VARIABLE).filter(|d| !d.is_empty()).map(PathBuf::from) else { return };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("{VARIABLE}: {}: {e}", dir.display());
        return;
    }
    eprintln!("{VARIABLE}: the windows go to {} every 2 s", dir.display());
    DIR.with(|d| *d.borrow_mut() = Some(dir));
    let target = crate::ui::target(mtm);
    // SAFETY: `snapshot:` is RPTarget's timer action; the run loop keeps the timer, the timer
    // keeps its target.
    let timer = unsafe {
        NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            2.0,
            &target,
            objc2::sel!(snapshot:),
            None,
            true,
        )
    };
    // SAFETY: the common modes constant; the main run loop.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
}

/// With the snapshots on, each command run goes to stderr too, so a test script's log shows
/// what its clicks reached.
pub fn log_command(cmd: crate::commands::Command) {
    if DIR.with(|d| d.borrow().is_some()) {
        eprintln!("{VARIABLE}: command {cmd:?}");
    }
}

/// Draws and lists the windows now (the list first: it is there even where drawing fails).
pub fn take() {
    let Some(dir) = DIR.with(|d| d.borrow().clone()) else { return };
    let Some(mtm) = MainThreadMarker::new() else { return };
    let windows: Vec<Retained<NSWindow>> =
        NSApplication::sharedApplication(mtm).windows().iter().filter(|w| w.isVisible()).collect();
    let mut list = String::new();
    for (i, w) in windows.iter().enumerate() {
        let f = w.frame();
        let _ = writeln!(list, "== window-{}: \"{}\" {}x{}", i + 1, w.title(), f.size.width, f.size.height);
        let mut texts = Vec::new();
        if let Some(frame_view) = frame_view(w) {
            texts_of(&frame_view, &mut texts);
        }
        for t in texts {
            let _ = writeln!(list, "  {}", t.replace('\n', "\n  "));
        }
    }
    let _ = std::fs::write(dir.join("windows.txt"), list);
    for (i, w) in windows.iter().enumerate() {
        let path = dir.join(format!("window-{}.png", i + 1));
        if let Err(e) = save_png(w, &path) {
            eprintln!("{VARIABLE}: window {}: {e}", i + 1);
        }
    }
    for stale in windows.len() + 1.. {
        if std::fs::remove_file(dir.join(format!("window-{stale}.png"))).is_err() {
            break;
        }
    }
}

/// The window's frame view (the title bar and toolbar with the content).
fn frame_view(w: &NSWindow) -> Option<Retained<NSView>> {
    let content = w.contentView()?;
    // SAFETY: the window keeps its frame view while it lives; we hold the window.
    unsafe { content.superview() }.or(Some(content))
}

fn save_png(w: &NSWindow, path: &std::path::Path) -> Result<(), String> {
    let view = frame_view(w).ok_or("no content view")?;
    let bounds = view.bounds();
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return Err("not laid out yet".into());
    }
    let rep = view.bitmapImageRepForCachingDisplayInRect(bounds).ok_or("no bitmap")?;
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let props: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
    // SAFETY: PNG with no properties.
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        .ok_or("no PNG")?;
    if data.writeToFile_atomically(&NSString::from_str(&path.to_string_lossy()), true) {
        Ok(())
    } else {
        Err(format!("cannot write {}", path.display()))
    }
}

/// The texts of the visible views under `view`, in view order.
fn texts_of(view: &NSView, out: &mut Vec<String>) {
    if view.isHidden() {
        return;
    }
    let any: &AnyObject = view.as_ref();
    if let Some(t) = any.downcast_ref::<NSTextField>() {
        let s = t.stringValue().to_string();
        if !s.is_empty() {
            out.push(s);
        }
    } else if let Some(p) = any.downcast_ref::<NSPopUpButton>() {
        out.push(format!("[{}]", p.titleOfSelectedItem().map(|s| s.to_string()).unwrap_or_default()));
    } else if let Some(b) = any.downcast_ref::<NSButton>() {
        let s = b.title().to_string();
        if !s.is_empty() {
            out.push(format!("[{s}]"));
        }
    } else if let Some(tabs) = any.downcast_ref::<NSTabView>() {
        let labels: Vec<String> = tabs.tabViewItems().iter().map(|i| i.label().to_string()).collect();
        out.push(format!("tabs: {}", labels.join(" | ")));
    }
    for v in view.subviews().iter() {
        texts_of(&v, out);
    }
}
