# Changelog archive: 2026-11-06, the macOS GUI's plan and wave 0 (v1.2.6-v1.2.8)

Moved from the root CHANGELOG.md on 2026-11-06_142501_1793971501. Paths and links keep their repository-root context.

## 2026-11-06_112615_1793960775: Version 1.2.8: the Mac GUI's wave 1 contracts

- Stubs for what wave 1's slices call in each other, wired into `app::apply` and Find, so each branch builds alone and the merges stay small: `dialogs::add::{handle, from_events}` (F), `detail::{refresh, find}` (D), `mac::{integration, set_integration, associations_settable}`, `mac::power::on_events`, `mac::update::on_events`, `mac::status_item::update` (G). Listed in MACOS_GUI.md "Wave 1 contracts". Clippy for both Apple targets and Windows, fmt.

## 2026-11-06_112123_1793960483: Version 1.2.7: `rookparcel-mac`, the native macOS GUI's foundation (slice B)

- New `crates/cocoa` (`rookparcel-mac`, D61) on objc2 0.6 / objc2-app-kit 0.3 (macOS-only, default features off; THIRD-PARTY-NOTICES rows added): the helper layer later slices build on (`RPTarget` command dispatch, `RPTableSource` virtual tables and outlines, `RPCanvas`, `ui/` controls, layout, alerts and panels, the full menu tree with Mac shortcuts, SF Symbols, D27 images, `ROOKPARCEL_SNAPSHOTS`), the main window shell, and a stub per later slice that logs "not yet". Other systems build a stub. 26 Mac menu strings in all 35 languages (machine translations; fa, sr, hr, bg, nb, pl, hu, it, zh-Hant and tr are the least sure).
- At the merge, the window runs on `frontend::app::Core` (slice A) instead of B's stand-in: its events drive the table, status bar, title and error sheets; the library is `Rookparcel/mac` itself (B had a `state/` subfolder), as GTK's layout, for the later takeover; `--demo` never asks the update server.
- `macos_bundle.py native` makes a signed `Rookparcel.app` (one binary or a `lipo` of two, macOS 11, 35 localizations) with dry-run tests; `tools/mac-vm/` pushes, builds, runs and pulls on the VM under a lock; CI lints the crate for x86_64 and type-checks it with Rust 1.85.
- Checked: Windows clippy (aarch64 and x86_64 Apple targets, Windows), fmt, the Mac and frontend tests, the bundle tests, the Win32 and nox builds; on the VM a demo of 2,000 torrents listed in 0.87 s at 0.0 % idle CPU, and a portable start on the real engine (library, lock, `rss.dat` and the first start's port in the folder, clean Quit). Slice B measured 100,000 rows in about 1 s warm and a 4.2 MB release app.
- **Issues:** the VM's console was locked, so Find, the Edit items, Minimize, Zoom, Full Screen and `screencapture` are unchecked (the owner unlocks it); AppKit's toolbar and frame autosaves write `~/Library/Preferences` even in portable mode (slice C keeps them in prefs); there is no 1024 px icon source, so the icns stops at 512 px.

## 2026-11-06_101504_1793956504: Version 1.2.6: `frontend::app`, the shared controller (macOS GUI slice A)

- `crates/frontend/src/app/`: `Core` is GTK's `App` without widgets: the alert loop, the 1 s tick (scheduler, seeding goals, auto-load, RSS, Auto Shutdown, update check), torrent commands, the add, magnet and URL flows, the feed view, import and `apply_options`. Dialogs, notifications and power actions come back as `Events` or return values; workers wake the GUI through `Start::wake`. `app::dirs` generalises GTK's `paths.rs` to any GUI folder (`gtk`, `mac`), with `lock_library`, `prepare_options` and `adopt_server`. GTK and Win32 are unchanged; nothing uses it until the Mac GUI.
- Deliberate differences from GTK: Logger times come from `Start::clock`; a magnet's dialog is marked open only once its metadata is there (GTK could leave the mark stuck); the detail tabs drop a removed torrent at once.
- Checked on the merge: the frontend tests (308, with the new `Core` tests on the mock engine), clippy (stable, and `aarch64-apple-darwin`), fmt, the Win32 and nox builds. `fuzz/Cargo.lock` was already behind (1.2.2) and is left to its next fuzz build.

## 2026-11-06_094234_1793954554: The native macOS GUI is planned (D61)

- [MACOS_GUI.md](docs/MACOS_GUI.md): the AppKit GUI of D52 (`crates/cocoa`, `rookparcel-mac`), its conventions, the VM workflow and briefs for slices A-L in four waves. The owner chose (D61): SF Symbols for toolbar and menus with D27 state icons, macOS 11 as the floor, a shared toolkit-free controller `frontend::app` ported from GTK's `app.rs`, instant-apply Preferences; the library moves to `Rookparcel/mac`, taking over the GTK bundle's `gtk` folders. Dependencies to add: the objc2 family (MSRV 1.71), macOS-only.
- Checked for the plan: the macOS VM (Intel 15.8.2, Command Line Tools only, rustc 1.99) and that Windows type-checks both Apple targets; objc2-app-kit 0.3.2 has the needed APIs (`sidebarWithViewController`, `searchMenuTemplate`, `reloadDataForRowIndexes`, the standard About panel, SF Symbols). D60, which sat in the "Proposed" table, is moved to the approved ones. Documentation only; no version bump.
