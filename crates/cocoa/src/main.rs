//! Rookparcel's native macOS GUI (D52, D61, docs/MACOS_GUI.md): µTorrent 2.2.1's main window
//! in AppKit through objc2, on the shared frontend models and the same engine as the Win32 and
//! GTK clients. Its library is `~/Library/Application Support/Rookparcel/mac` (`paths`).
//!
//! `--demo N` runs the simulated engine with N torrents (screenshots, scale tests); `--version`
//! prints the build and exits; `--profile NAME` runs a profile of its own (D42).
//!
//! On other systems this is a stub that says so and exits 1, so the workspace builds and lints
//! everywhere.

// Slice B is the helper layer later slices build on (docs/MACOS_GUI.md): until they use it,
// much of it has no caller yet. On other systems only the pure modules' tests use theirs.
#![allow(dead_code)]

#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod canvas;
mod commands;
#[cfg(target_os = "macos")]
mod detail;
#[cfg(target_os = "macos")]
mod dialogs;
#[cfg(target_os = "macos")]
mod images;
#[cfg(target_os = "macos")]
mod import;
#[cfg(target_os = "macos")]
mod list;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod mainq;
mod menu;
mod paths;
#[cfg(target_os = "macos")]
mod prefs;
#[cfg(target_os = "macos")]
mod profiles;
#[cfg(target_os = "macos")]
mod remote;
#[cfg(target_os = "macos")]
mod rss;
#[cfg(target_os = "macos")]
mod search;
#[cfg(target_os = "macos")]
mod search_helper;
#[cfg(target_os = "macos")]
mod setup_guide;
#[cfg(target_os = "macos")]
mod sidebar;
#[cfg(target_os = "macos")]
mod snapshots;
#[cfg(target_os = "macos")]
mod statusbar;
mod symbols;
#[cfg(target_os = "macos")]
mod table;
#[cfg(target_os = "macos")]
mod torrent_menu;
mod ui;
#[cfg(target_os = "macos")]
mod window;

/// `--demo N`, if given.
fn demo_arg(args: &[String]) -> Option<usize> {
    let i = args.iter().position(|a| a == "--demo")?;
    args.get(i + 1).and_then(|n| n.parse().ok())
}

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use std::process::ExitCode;

    use objc2::MainThreadMarker;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use rookparcel_frontend::prefs::Prefs;
    use rookparcel_frontend::{fmt, lang, profile, tr};

    let started = std::time::Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // before AppKit, so it needs no window server (the release job runs it)
    if matches!(args.first().map(String::as_str), Some("--version" | "-V")) {
        println!("rookparcel-mac {}", rookparcel_frontend::BUILD);
        return ExitCode::SUCCESS;
    }
    let Some(mtm) = MainThreadMarker::new() else { return ExitCode::FAILURE };
    // first: the folders depend on it (D42)
    let bad_profile = match profile::split_args(&args) {
        Ok((name, _)) => {
            profile::init(name);
            None
        }
        Err(e) => Some(e),
    };
    // PaTorrent's folders become ours before anything reads them (D59)
    let still_patorrent = rookparcel_frontend::legacy::migrate().err();
    let prefs = Prefs::load(paths::data_dir().join("rookparcel.ini"));
    fmt::set_utc_offset(objc2_foundation::NSTimeZone::localTimeZone().secondsFromGMT() as i32);
    for p in lang::init(prefs.get("ui.language").unwrap_or_default()) {
        eprintln!("{p}");
    }
    set_appkit_language(lang::current(), lang::right_to_left());

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    // SAFETY: the icon is a valid image; an app started outside its bundle shows it in the Dock.
    unsafe { app.setApplicationIconImage(Some(&images::app_icon(128.0))) };
    if let Some(e) = bad_profile {
        ui::alert(ui::Style::Error, &tr!(ProfileBadName, name = e.given()), "", &[], mtm);
        return ExitCode::from(2);
    }
    if let Some(old) = still_patorrent {
        ui::alert(ui::Style::Error, &tr!(FolderInUse, folder = old.display()), "", &[], mtm);
        return ExitCode::FAILURE;
    }
    let delegate = app::RPAppDelegate::new(app::Args { demo: demo_arg(&args), started }, mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    ExitCode::SUCCESS
}

/// AppKit's own texts (panels, the text menus, Full Screen) and its right-to-left layout follow
/// Rookparcel's language: `AppleLanguages` in the volatile argument domain, before
/// `NSApplication` starts.
#[cfg(target_os = "macos")]
fn set_appkit_language(code: &str, rtl: bool) {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{
        NSArgumentDomain, NSArray, NSMutableDictionary, NSNumber, NSString, NSUserDefaults,
    };

    let defaults = NSUserDefaults::standardUserDefaults();
    // SAFETY: the argument domain's name constant.
    let domain = unsafe { NSArgumentDomain };
    let d: Retained<NSMutableDictionary<NSString, AnyObject>> = NSMutableDictionary::new();
    let old = defaults.volatileDomainForName(domain);
    d.addEntriesFromDictionary(&old);
    let set = |key: &str, value: Retained<AnyObject>| d.insert(&*NSString::from_str(key), &*value);
    let langs = NSArray::from_retained_slice(&[NSString::from_str(code)]);
    set("AppleLanguages", Retained::into_super(Retained::into_super(langs)));
    if rtl {
        let yes =
            || Retained::into_super(Retained::into_super(Retained::into_super(NSNumber::new_bool(true))));
        set("AppleTextDirection", yes());
        set("NSForceRightToLeftWritingDirection", yes());
    }
    // SAFETY: a dictionary of property-list values.
    unsafe { defaults.setVolatileDomain_forName(&d, domain) };
}

#[cfg(not(target_os = "macos"))]
fn main() {
    let _ = demo_arg(&[]);
    eprintln!(
        "rookparcel-mac is the GUI for macOS; on Windows use rookparcel.exe, elsewhere rookparcel-gtk."
    );
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    #[test]
    fn demo_argument() {
        let a = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(super::demo_arg(&a(&["--demo", "100000"])), Some(100000));
        assert_eq!(super::demo_arg(&a(&["--profile", "x", "--demo", "5"])), Some(5));
        assert_eq!(super::demo_arg(&a(&["--demo"])), None);
        assert_eq!(super::demo_arg(&a(&[])), None);
    }
}
