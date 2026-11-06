# Native macOS GUI (D52, D61): `rookparcel-mac`

Planned 2026-11-06_094234_1793954554. This is the design, the rules every slice follows and one brief per slice. An agent reads **Conventions**, **Test and VM workflow** and its own slice section; the rest is background.

## Why

The GTK bundle is today's macOS GUI (D23): about 100 MB, arm64 only, macOS 15 at least, never checked on a real Mac. D52 (owner, 2026-11-04) approved a native AppKit GUI on the shared frontend model with `objc2-app-kit` as the binding; it replaces the GTK bundle on macOS once it covers every dialog. D61 (owner, 2026-11-06) settles its shape:

- SF Symbols for the toolbar and menus (menu-item symbols only on macOS 26 and later, where the system's own menus carry them); the D27 bold family for torrent states and sidebar categories.
- Minimum macOS 11 Big Sur.
- A shared toolkit-free controller, `frontend::app`, ported from GTK's `app.rs`; GTK keeps its own copy for now (BACKLOG).
- Preferences apply instantly, Mac style: no OK, Cancel or Apply.
- The library moves to `Application Support/Rookparcel/mac`, taking over the GTK bundle's `gtk` folders once; the bundle id stays `io.github.balls0fsteel.Rookparcel`.
- One universal dmg (arm64 and x86_64).

Target: a single universal binary of about 10 MB, native controls everywhere, 100,000 torrents with a responsive list and idle CPU like the Win32 GUI's (1.16 s to list, 49 ms name sort, 0.05-0.10 % idle on the development PC).

## Facts

- **The Mac VM** is the owner's own machine: Intel macOS 15.8.2 under VMware, `ssh -i ~/.ssh/patorrent_vm x@192.168.1.128`, 4 cores, 8 GB, passwordless sudo, auto-login. Command Line Tools only (SDK 15.5): no Xcode, so no `ibtool`/`actool`, no NIBs. rustc 1.99.1, target x86_64 only. `iconutil`, `sips`, `codesign`, `hdiutil`, `lipo`, `plutil`, `python3` present. Over SSH, Accessibility (osascript UI scripting) works; `screencapture -x` and anything needing a key window (Find, Edit items, Minimize, Zoom, Full Screen, typed keys) only while the console session is unlocked (`ioreg -n Root -d1 | grep CGSSessionScreenIsLocked`), which needs the owner; the app's own `ROOKPARCEL_SNAPSHOTS` work either way (`tools/mac-vm/README.md`). It cannot run arm64 code or show Retina.
- **Windows** has `aarch64-apple-darwin` (toolchains 1.90 and stable 1.97) and `x86_64-apple-darwin` (1.90): the Mac crate type-checks and lints here; nothing links.
- **Crates** (all MSRV 1.71): objc2 0.6.5 (MIT), objc2-foundation 0.3.2, objc2-app-kit 0.3.2, block2 0.6.2, objc2-user-notifications 0.3.2, objc2-uniform-type-identifiers 0.3.2 (Zlib, Apache-2.0 or MIT). objc2-app-kit's default features turn on all 300+ classes and a dozen framework crates: use `default-features = false` and list what is used. objc2's `build.rs` is pure Rust. `available!` is in objc2 0.6.
- **No app model in the frontend:** GTK's `App` (`crates/gtk/src/app.rs:56`) mixes the alert loop, the 1 s tick, the scheduler, seeding goals, auto-load, RSS, applying options and the add/remove/label/pause logic with widgets. Slice A moves that logic into `frontend::app`.
- **Reused as they are:** `model::TorrentList`, `columns`, `status`, `detail::Detail`, `pref_pages::{pages, Form, check, page_of}`, `add`, `create`, `properties`, `rss`, `search` (with `helper`), `import`, `profile`, `remote`, `setup_guide`, `closing`, `auto_shutdown`, `update`, `icons`, `lang`, `legacy`, `integration`; core's `Session::start`, `Session::start_mock` (feature `mock`), `remote::connect`, `util::lockfile`; GTK `macos.rs`'s pure LaunchAgent plist code; `.github/scripts/macos_bundle.py` (`info_plist()`, the icns, `--dry-run`).

## Architecture

### Crate

- `crates/cocoa`, package and binary `rookparcel-mac`, a workspace member. On other targets `main.rs` is a stub that prints that it is for macOS and exits 1 (as the GTK crate does on Windows), so workspace builds and clippy pass everywhere. Every objc2 crate is under `[target.'cfg(target_os = "macos")'.dependencies]`.
- Dependencies: `rookparcel-core` (feature `mock`, for `--demo N`), `rookparcel-frontend`, `objc2`, `objc2-foundation`, `objc2-app-kit`, `block2` (completion handlers only), `objc2-user-notifications`, `objc2-uniform-type-identifiers`. Features one per line, sorted, so parallel additions merge cleanly. Main-queue dispatch is a few lines of `dispatch_async_f` FFI to libSystem (no `dispatch2`).
- No Swift, NIB, storyboard or Xcode project. The UI is built in code with Auto Layout.
- The bundle: `Rookparcel.app/Contents/MacOS/Rookparcel`, a universal binary (`lipo`, `MACOSX_DEPLOYMENT_TARGET=11.0`), `Info.plist` with `NSPrincipalClass` = `NSApplication`, `LSMinimumSystemVersion` 11.0, `CFBundleLocalizations` (the 35 languages), the existing document/URL types and `NSLocalNetworkUsageDescription`; ad-hoc signed as today.

### Files (slice B creates every one; later slices fill their own)

| File | Owner | Role |
|---|---|---|
| `main.rs` | B | entry, stub elsewhere, arguments, profile, `legacy::migrate`, prefs, language, NSApplication |
| `app.rs` | B, then C | `App { core, ui }`, `with_core`, `ui()`, `run(Command)`, `state(Command)`, alert and tick glue |
| `mainq.rs` | B | `on_main(f)` |
| `commands.rs` | B | the `Command` enum (every action), tags |
| `menu.rs` | B | the main menu tree, shortcuts, Dock and status-item menus |
| `ui/` | B | strings, controls, stacks, grids, alerts, panels, `RPTarget` |
| `table.rs` | B | `RPTableSource`: generic virtual NSTableView / NSOutlineView |
| `canvas.rs` | B | `RPCanvas`: drawn views |
| `symbols.rs` | B | SF Symbols names per command, page and tab |
| `images.rs` | B | NSImage from `frontend::icons` PNGs (16 and 32 px reps, dark variants) |
| `snapshots.rs` | B | `ROOKPARCEL_SNAPSHOTS` |
| `window.rs` | B, then C | main window: toolbar, split views, table, tabs, status bar |
| `sidebar.rs`, `list.rs`, `statusbar.rs`, `torrent_menu.rs` | C | main window content and torrent commands |
| `detail/` (`mod.rs`, `general.rs`, `tables.rs`, `pieces.rs`, `files.rs`, `speed.rs`) | D | detail tabs |
| `prefs/` (`mod.rs`, `controls.rs`, `advanced.rs`, `sched_grid.rs`) | E | Preferences |
| `dialogs/` (`add.rs`, `add_url.rs`, `create.rs`, `properties.rs`, `input.rs`) | F | torrent dialogs |
| `mac/` (`lifecycle.rs`, `closing.rs`, `status_item.rs`, `dock.rs`, `notify.rs`, `open.rs`, `login.rs`, `assoc.rs`, `power.rs`, `about.rs`, `update.rs`) | G | system integration |
| `rss.rs` | H | RSS |
| `search.rs`, `search_helper.rs` | I | search |
| `import.rs`, `profiles.rs`, `remote.rs`, `setup_guide.rs` | J | profiles and setup |

### Threading and state

- AppKit runs on the main thread only. Get a `MainThreadMarker` at entry and pass it down.
- State: `thread_local! APP: RefCell<Option<App>>`, `App { core: frontend::app::Core, ui: Ui }`, reached through `with_core(|c| ..)` and `ui()`, as GTK's `with_app` and `ui()`.
- **Re-entrancy:** AppKit calls back synchronously (`reloadData` calls the data source at once, `makeKeyAndOrderFront` may call the delegate). Compute under the borrow, drop it, then call AppKit. Never call an AppKit mutator while `APP` is mutably borrowed. Data sources take a shared borrow only.
- Engine wakeups: `handle.set_alert_notify(|| on_main(app::on_alerts))`, then `on_alerts()` once (alerts queued during the start do not fire the notify). Workers (RSS, search, update, URL fetch) wake through `on_main` too.
- The 1 s tick is an `NSTimer` in the run loop's common modes, so it fires during menu tracking and resizing; the main dispatch queue drains in those modes too.

### `frontend::app` (slice A): the contract for every later slice

`Core` is GTK's `App` without widgets, ported with the same behaviour:

```rust
pub struct Start { pub session: Session, pub remote: Option<RemoteInfo>, pub prefs: Prefs,
                   pub options: Options, pub dirs: Dirs, pub wake: Arc<dyn Fn(Wakeup) + Send + Sync> }
pub enum Wakeup { Alerts, Rss, Update, Fetched }
impl Core {
    pub fn start(s: Start) -> Core;                 // GTK build(): sort, IP filter, adapters, notify, first drain
    pub fn on_alerts(&mut self) -> Events;          // GTK on_alerts() and refresh()'s model part
    pub fn tick(&mut self, now: Instant) -> Events; // the 1 s timer body; detail requests for the shown tab
    pub fn on_wake(&mut self, w: Wakeup) -> Events; // rss_collect, update_done, a fetched .torrent
    pub fn select(&mut self, ids: &[TorrentId]);  pub fn show_tab(&mut self, tab: Tab);
    pub fn status_bar(&self) -> StatusBar;  pub fn title(&self) -> String;  pub fn tray_tip(&self) -> String;
    pub fn menu_state(&self, ids: &[TorrentId]) -> MenuState;  pub fn labels(&self) -> Vec<String>;
    pub fn torrent_action(&mut self, a: TorrentAction, ids: &[TorrentId]);
    pub fn remove(..); pub fn set_label(..); pub fn set_limit(..); pub fn toggle_super_seeding(..);
    pub fn set_download_location(..);
    pub fn add_file(&mut self, bytes: Vec<u8>) -> AddStep;  // Dialog(AddRequest) | Added | AlreadyInList(name)
    pub fn add_magnet(&mut self, uri: String) -> AddStep;  pub fn add_url(&mut self, url: String);
    pub fn finish_add(&mut self, r: AddRequest, c: Option<AddChoice>);
    pub fn next_magnet_dialog(&mut self) -> Option<AddRequest>;  pub fn finish_magnet(..);
    pub fn apply_options(&mut self, o: &Options) -> Events; // diffs old and new as GTK apply_options does
    pub fn shutdown(&mut self) -> Exit;             // save prefs and rss, clear the notify, begin_shutdown
    // and: feed view (show_feed, feed rows, feed commands), Auto Shutdown step/act, import, update offer
}
pub struct Events { pub rows: bool, pub view: bool, pub sidebar: bool, pub detail: bool, pub status: bool,
                    pub title: bool, pub errors: Vec<(String, String)>, pub infos: Vec<String>,
                    pub completed: Vec<String>, pub magnet_ready: bool, pub remote_lost: bool,
                    pub auto_shutdown: Option<Step>, pub power: Option<Power>, pub update: Option<UpdateOffer> }
```

- `app::dirs(gui, env)` gives `Dirs { config, settings, data }`, generalised from `crates/gtk/src/paths.rs` (profiles, portable, XDG, macOS `Application Support/Rookparcel/<gui>`), with the environment passed in for tests.
- Later slices may **add** to `Core` (new methods, new `Events` fields with defaults), each with a test; they do not change what is there.

### Main window: the 2.2.1 layout in AppKit

- `NSWindow` with an `NSToolbar` (customizable, autosaved; SF Symbols 2 names): Add Torrent, Add URL, Create | Remove | Start, Pause, Stop | Queue Up, Queue Down | RSS | flexible space | search. The search is an `NSSearchField` whose `searchMenuTemplate` is the mode menu (Search torrent names, each source, All sources, Manage Sources...).
- `NSSplitViewController`: a sidebar item (source-list `NSOutlineView`: Torrents with Downloading, Completed, Active, Inactive; Labels with No Label and each label; Feeds with each feed; counts; D27 category icons) and a content item, a vertical `NSSplitView` with the torrent table over an `NSTabView` of the 7 detail tabs.
- The torrent table: a view-based `NSTableView` over `list.view()`. Cells are reused (`makeViewWithIdentifier`). Each second only the visible rows reload (`reloadDataForRowIndexes:columnIndexes:` over `rowsInRect(visibleRect)`); `reloadData` only when `view_generation()` changed, restoring the selection by `TorrentId`. The 24 columns from `frontend::columns`; the header menu shows and hides them (Name stays); sort descriptors drive `list.set_sort`; widths, order and visibility are kept in prefs (`prefs.columns()`), not AppKit autosave, so portable mode keeps them. Done is a drawn bar (blue downloading, green complete) with the percentage.
- The status bar: the window's bottom content border (`setContentBorderThickness:forEdge:`) with a horizontal stack: the update link, the network icon and text, DHT, down, up; tooltips as GTK.
- `RPCanvas` draws every custom view (piece bars, piece map, block bar, speed graph, Done cells, the scheduler grid) with `NSBezierPath` and semantic `NSColor`s, redraws on `viewDidChangeEffectiveAppearance`, and sets an accessibility label and value.

### Menus and shortcuts

Ctrl becomes ⌘; Mac conventions win where they clash.

- App menu: About Rookparcel, Check for Updates (when available) | Preferences ⌘, | Setup Guide ⌘G | Services | Hide ⌘H, Hide Others ⌥⌘H, Show All | Quit ⌘Q.
- File: Add Torrent ⌘O, Add Torrent from URL ⌘U, Create New Torrent ⌘N | Import from qBittorrent, Import from µTorrent | Profiles ▸ (list with the current checked, New Profile, Open Profile Folder) | Connect to Remote | Close Window ⌘W.
- Edit: Undo ⌘Z, Redo ⇧⌘Z | Cut ⌘X, Copy ⌘C, Paste ⌘V, Delete, Select All ⌘A (first-responder actions, nil target) | Find ⌘F (the toolbar search; in the Files tab its filter).
- View: Show Toolbar ⌥⌘T, Customize Toolbar | Show Sidebar ⌃⌘S, Show Detailed Info, Show Status Bar (AppKit adds Enter Full Screen).
- Torrent: Open Containing Folder | Force Start, Start ⌘S, Pause ⇧⌘P, Stop ⌘T | Queue Up, Down, Top, Bottom | Remove ⌘⌫ (⌫ in the list), Remove And Delete .torrent + Data ⌥⌘⌫ | Force Re-check, Update Tracker | Advanced ▸, Bandwidth ▸, Label ▸ | Properties ⌘I (Return in the list). The list's context menu is the same menu.
- Options: RSS Downloader | Auto Shutdown ▸.
- Window: Minimize ⌘M, Zoom | Bring All to Front.
- Help: Rookparcel Website | Third-Party Notices.
- Dock menu and status-item menu: Show/Hide | Add Torrent, Add Torrent from URL | Start All, Stop All | Quit.
- Strings: the Mac-only menu texts (Edit, View, Window and the app menu, about 25) are added once by slice B to `en.rs` and machine-translated into all 34 tables. Every other slice reuses existing keys.

### Preferences: instant apply

- One window: a sidebar of the pages from `pref_pages::pages(Features { tray: true, desktop: true, packaged: false, updates, alternate_rows: true })`, each with an SF Symbol and the page title (System Settings style); the page is an `NSGridView` in a scroll view; the window title follows the page.
- A control commits: check boxes and pop-ups at once; text and number fields on Return, losing focus, and the window closing; folder and file fields after their panel.
- On commit, `Form::read(&pages, &applied, |k| text_of(k))`: `Ok(o)` goes to `core.apply_options(&o)` (which diffs, stores and saves), then the greying is redone with `Field::enabled`; `Err((key, msg))` shows the message in red under that field and keeps the last good value applied; closing the window with a bad field puts back that field's applied text.
- Help (D55): every control's tooltip from `item_help`; a help line at the page's bottom for the focused control; a help button when `help::wiki_url` is `Some` (it is `None` today).
- The Advanced page: a filterable name/value table (`advanced_rows`, `advanced_name`, `help::for_setting`) committing on edit. The Scheduler page: the grid on `RPCanvas` (`scheduler::{hover_text, drag_line, Mode::rgb}`).
- Associations and autostart go through slice G's `mac::assoc` and `mac::login`.

### macOS integration

- **Opening:** `application:openURLs:` takes files and `magnet:` links; those that come before `Core` exists wait in a pending list (GTK's `PENDING`). Dropping `.torrent` files on the window or the Dock icon opens them too.
- **A second start of the same profile:** the library lock is held, so the new process posts `io.github.balls0fsteel.Rookparcel.open` on `NSDistributedNotificationCenter` (object: the profile name; user info: its arguments) and exits 0; the running copy opens them and activates.
- **Close and quit:** the close button follows `ui.close_to_tray` (default on): the window hides, and the Dock icon (`applicationShouldHandleReopen:`) or the status item brings it back; minimize-to-tray likewise; `--minimized` starts hidden. Quit and logout go through `applicationShouldTerminate:` → `NSTerminateLater`, `core.shutdown()`, the D47 closing window (`frontend::closing`), `replyToApplicationShouldTerminate:` once `Exit` is done; power-off shows the closing window at once.
- **Status item:** `NSStatusItem` with GTK `tray_macos.rs`'s menu, clicks and tooltip, rewritten with objc2; the colour icon (there is no monochrome rook).
- **Notifications:** completed downloads (`ui.balloons`) through `UNUserNotificationCenter`; if the VM shows an ad-hoc-signed app refused, `NSUserNotificationCenter` instead.
- **Login:** the per-profile LaunchAgent with GTK's labels and program path, so existing agents keep working. The pure plist functions move from `crates/gtk/src/macos.rs` to `frontend::integration` (GTK re-exports them).
- **Associations:** LaunchServices `LSSetDefaultRoleHandlerForContentType` / `LSSetDefaultHandlerForURLScheme` and their Copy functions, as GTK does (works from 11 to 15); only from the bundle and in the default profile; the startup check asks as GTK does.
- **Auto Shutdown:** Quit, plus Sleep and Shut Down by Apple events to loginwindow (Apple's QA1134: `kAESleep`, `kAEShutDown`), honouring `ROOKPARCEL_POWER_DRY_RUN`; Hibernate greyed.
- **About** is `orderFrontStandardAboutPanelWithOptions:`; Third-Party Notices a text window; the update check a status-bar link and an `NSAlert` opening the release page; the translocation notice `MacTranslocated`.
- **Library:** `Application Support/Rookparcel/mac` and `profiles/<name>/mac`. At the first start, a `gtk` folder whose `mac` sibling is absent and whose lock is free is renamed to `mac`, with paths inside its `rookparcel.ini` rewritten as `legacy::migrate` does for PaTorrent.
- **Language:** `lang::init` as usual; before `NSApplication` starts, the volatile argument domain gets `AppleLanguages = [ui.language]`, so AppKit's own texts (panels, text menus, Full Screen) and right-to-left layout follow the app's language.
- **Appearance:** semantic colours; D27 dark PNGs by effective appearance; state icons from the 16 and 32 px PNGs as two reps of one 16 pt image, sharp on Retina.

## Conventions

1. Few Objective-C classes, `define_class!`, `RP` prefix: `RPAppDelegate`; `RPTarget` (one action target: `perform:` maps the sender's tag to a `Command`; `validateMenuItem:` and `validateToolbarItem:` ask `app::state(cmd) -> (enabled, Option<checked>)`); `RPTableSource` (one data source and delegate, keyed by a `TableKind`); `RPCanvas`; `RPWindowDelegate`. Ask before adding another.
2. Every action is one `Command` value, run in one place (`app::run`). Menus, toolbar, buttons, the status item and the Dock all use it.
3. Text through `ui::text(tr!(..))` (strips `&`, turns `...` into `…`); never hard-coded English.
4. `unsafe` only where objc2 requires it, each block with a `// SAFETY:` comment.
5. macOS 11 is the floor: any newer API behind `available!(macos = N)`, SF Symbols only from the SF Symbols 2 set (or a fallback).
6. Semantic colours only; no fixed frames: Auto Layout stacks and grids, so translations and RTL fit.
7. Logic goes in `frontend` with tests that run on Windows; the Mac crate stays a view layer.
8. Reuse existing strings. A new key goes in `en.rs` (end of its group) and `en.lng` and gets a machine translation in all 34 `.lng` files (`every_table_is_shipped_and_complete`); `ROOKPARCEL_WRITE_LNG=1 cargo test -p rookparcel-frontend lang::` regenerates `en.lng`.
9. No version bump, no edits to `PLAN.md` or `CHANGELOG.md`: the report carries the changelog text. Edit only your slice's files, plus the shared touch points: sorted feature lines in `crates/cocoa/Cargo.toml`, the `lang` files, and additions to `frontend::app`.
10. Builds and tests at low priority on few cores: on Windows `(Get-Process -Id $PID).PriorityClass = 'Idle'` in the same command, `-j 1`, `--test-threads=1`; on the VM `-j 1`.
11. MSRV 1.85: no let-chains.

## Test and VM workflow

**On Windows, before every report:**

```powershell
(Get-Process -Id $PID).PriorityClass = 'Idle'; cargo +stable clippy -j 1 -p rookparcel-mac --all-targets --target aarch64-apple-darwin -- -D warnings
(Get-Process -Id $PID).PriorityClass = 'Idle'; cargo clippy -j 1 -p rookparcel-mac --all-targets --target x86_64-apple-darwin -- -D warnings
cargo fmt --all --check
(Get-Process -Id $PID).PriorityClass = 'Idle'; cargo test -j 1 -p rookparcel-frontend -- --test-threads=1
```

**On the VM** (`tools/mac-vm/`, slice B):

- `push.sh SLICE` (Git Bash on Windows): tars `git ls-files` (with `assets/`, whose icons are embedded; without `docs/`, `site/`, `third_party/`), copies it with scp and unpacks it into `~/rp/SLICE`.
- `build.sh SLICE` (on the VM): takes the `~/rp/vm.lock` lock (`mkdir`, owner and time inside, stale after 30 min), `CARGO_TARGET_DIR=~/rp/target cargo build -j 1 -p rookparcel-mac`, then `macos_bundle.py native` makes a dev `Rookparcel.app` in `~/rp/SLICE/out`.
- `run.sh SLICE [seconds] [osascript file] [-- app arguments]`: a portable copy (the app and an empty `rookparcel.ini` in `~/rp/SLICE/run`, so nothing goes to `~/Library`), `ROOKPARCEL_SNAPSHOTS=~/rp/SLICE/shots`, the script (menus by name through System Events), `screencapture -x`, then Quit.
- `pull.sh SLICE DIR`: copies the shots back.
- `ROOKPARCEL_SNAPSHOTS=<dir>`: every 2 s each window's frame view as `window-<n>.png` (`cacheDisplayInRect:toBitmapImageRep:`), and `windows.txt` with the titles and every text field, button and visible cell.
- `--demo N`: a mock session (`Session::start_mock`, as Win32), for screenshots and the 100k run.

The VM is the only machine an agent copies to. One VM run at a time (the lock); type-check on Windows first. Look at your screenshots before reporting.

**Report:** the files changed; what was checked on Windows and on the VM, with screenshot paths; what is not done or not checked; new strings; the changelog text.

## Slices

Waves run in order; slices in a wave run in parallel in their own worktrees. My merge after each branch: resolve (lang add/add conflicts: keep both, `ROOKPARCEL_WRITE_LNG=1` test twice, `help::` too when help texts changed), bump the patch version (D37), Windows checks, one VM run whose screenshots I look at, changelog and plan, one merge commit.

### Wave 0

#### A. Controller (`crates/frontend/src/app/`, Windows only)

- **Mirror:** `crates/gtk/src/app.rs`: `App` (56), `build` (895, the model part), `on_alerts` (1676), `refresh` (1832, the model part), the timer body in `build`, `torrent_action` (2220), `set_label`, `set_limit`, `toggle_super_seeding`, `set_download_location`, `set_paused`, `remove`, `add_torrent_file`, `add_magnet`, `magnet_dialogs`, `add`, `add_to_session`, `autoload`, `run_scheduler`, `seeding_goal`, `in_list`, `labels`, `save_prefs`, `download`, `apply_options` (3121), `load_ip_filter`, the RSS functions (3311-3556, model part), the update check (3213-3300, model part), `shutdown` (1641), `sync_title`, `update_status` (texts), `menu_state`, `super_seed_state`, `import_from` (logic); `crates/gtk/src/paths.rs` for `dirs`.
- **Own:** `crates/frontend/src/app/` (split it: `mod.rs`, `alerts.rs`, `tick.rs`, `commands.rs`, `add.rs`, `rss.rs`, `status.rs`, `dirs.rs`), `crates/frontend/src/lib.rs` (the module line).
- **Not:** GTK and Win32 stay as they are.
- **Done when:** the API above exists with the behaviour of GTK's code; tests on a mock session (`Session::start_mock` behind a dev-dependency feature, or the fakes the frontend tests already use) cover the first-sight label and pause marks, `TorrentAdded` with "top of queue", `TorrentRemoved` clearing prefs, `TorrentFinished` only for downloads seen, magnets awaiting metadata, the scheduler's seed-only mode, seeding goals, `apply_options` side effects (PEX, IP filter, scheduler), the status-bar and title texts, and `dirs` for default, profile and portable on macOS and Linux paths; the Windows checks pass.

#### B. Foundation (`crates/cocoa/`, bundling, VM tools)

- **Mirror:** `crates/gtk/src/main.rs`, `app.rs` (`run`, `startup`, `build`'s widget part), `list.rs`, `table.rs`, `canvas.rs`, `snapshots.rs`, `tray_macos.rs` (how the objc runtime is driven today); `crates/win32/src/main.rs` (`--demo`); `.github/scripts/macos_bundle.py`; `.github/workflows/ci.yml`.
- **Own:** the whole of `crates/cocoa/` (with a stub file for each later slice: a public function per entry point that logs "not yet" through the Logger), the workspace `Cargo.toml` member line and `[workspace.dependencies]` for objc2, `.github/scripts/macos_bundle.py` (a `native` subcommand: `Info.plist` as above, icns including 1024 px, one binary or a `lipo` of two, `--dry-run` tests), `tools/mac-vm/` (scripts and a README), `ci.yml` (the macOS job builds and lints the crate for both architectures; the Ubuntu MSRV job adds `rustup target add aarch64-apple-darwin` and `cargo check -p rookparcel-mac --target aarch64-apple-darwin`), the Mac menu strings in `lang`, a THIRD-PARTY-NOTICES draft for the objc2 crates (in the report).
- **Build:** `main.rs` (non-mac stub; on macOS arguments, `profile::split_args`/`init`, `legacy::migrate`, prefs, `fmt::set_utc_offset`, `lang::init`, the `AppleLanguages` argument domain, `NSApplication` with the regular activation policy and `RPAppDelegate`), `app.rs` (state, `run`, `state`, a temporary `--demo` loop: a mock session's `StateUpdate`s into a `TorrentList`, replaced by `Core` at merge), `mainq.rs`, `commands.rs` (every action of the menus above), `menu.rs` (the full tree with shortcuts; unbuilt actions log "not yet"), `ui/`, `table.rs` (columns from a spec list, header menu, sort descriptors, selection, double-click, Return and ⌫ through a small `NSTableView` subclass or `keyDown:` hook, context menu, `reload_visible`, `reload_keep_selection`), `canvas.rs`, `symbols.rs`, `images.rs`, `window.rs` (toolbar with the search field and its menu template, split view controller with an empty sidebar outline, the torrent table, the tab view with 7 empty tabs, the status bar), `snapshots.rs`.
- **Done when:** on the VM, `--demo 100000` shows the window with toolbar, sidebar, a scrolling, sortable table of 100,000 mock torrents, empty tabs and status bar, in light and dark mode; every menu opens (osascript) and every item either works (Quit, Hide, Minimize, Full Screen, Edit items in the search field) or logs "not yet"; `ROOKPARCEL_SNAPSHOTS` writes PNGs and `windows.txt`; `macos_bundle.py native` makes an app that `codesign --verify` accepts; the Windows checks and `macos_bundle.py`'s dry-run tests pass; the report has the measured list time and idle CPU for 100,000 rows.

### Wave 1 (after A and B are merged)

#### Wave 1 contracts (1.2.8)

The functions wave 1's slices call in each other exist as stubs, already wired, so every branch builds on its own. Fill your own; call the others as they are; do not change their signatures (if one does not fit, say so in the report).

| Function | Filled by | Called by |
|---|---|---|
| `dialogs::add::handle(AddStep)`: the dialog for `Dialog`, a message for `AlreadyInList` and `Error` | F | C (dropped files), G (opened files and links) |
| `dialogs::add::from_events(magnet_ready, adds)` | F | `app::apply` |
| `dialogs::input::ask(title, prompt, initial, done)` | F | C (New Label), J later |
| `detail::refresh()`: redraw the shown tab from `Core::detail` | D | `app::apply` (`Events::detail`), C after a selection change |
| `detail::show_tab(Tab)` | D | the window's tab delegate |
| `detail::find() -> bool`: the Files filter takes Find | D | `app::run(Find)` |
| `prefs::open()`, `prefs::open_at(key)` | E | the menus, C (the status bar's network click) |
| `mac::integration()`, `mac::set_integration(old, new)`, `mac::associations_settable()` | G | E (Preferences > General) |
| `mac::power::on_events(..)`, `mac::update::on_events(..)`, `mac::status_item::update()`, `mac::notify::completed(..)` | G | `app::apply` |
| `mac::open::open(urls)` | G | `RPAppDelegate` |

Slice C owns the rest of `app.rs` and `window.rs`: keep `apply`'s one-line-per-slice calls as they are. A slice that needs another hook adds one line and names it in its report.

#### C. Main window

- **Mirror:** GTK `app.rs` (`build`'s window part, `refresh`, `selection_changed`, `update_status`, `add_actions`, `torrent_menu`, `menu_state`, `remove`, `open_folder`, `set_download_location`, `set_label`, `set_limit`, `popup_menu`), `list.rs`, `sidebar.rs`; Win32 `app.rs` for the View toggles' prefs keys.
- **Own:** `app.rs` (wiring `Core` if the merge did not finish it), `window.rs`, `sidebar.rs`, `list.rs`, `statusbar.rs`, `torrent_menu.rs`.
- **Done when:** the table shows `Core`'s list with all 24 columns, state icons, the Done bar, sorting, column show/hide and layout kept in prefs; the sidebar shows categories and labels with counts (Feeds is a hook that slice H fills: `rss::sidebar_rows`, `rss::show_feed`); selection drives `core.select`; the search field filters by name (other modes call `search::run`, filled by slice I); the status bar and title follow `Core` (the network part's click opens Preferences > Connection while the port is taken); every Torrent menu and context menu item works, with Remove's confirmation, labels (New Label through `dialogs::input`), bandwidth limits, Initial Seeding, Set Download Location (`NSOpenPanel`), Open Containing Folder (`NSWorkspace`); double-click opens a finished torrent's folder and otherwise Properties, Return opens Properties, ⌫ removes; dropped `.torrent` files are added; the View toggles work and are kept; the toolbar configuration and the window frame are kept in prefs, not in AppKit's autosave (`NSToolbar` autosave and frame autosave write `~/Library/Preferences` even in portable mode: turn them off); the VM shows it with a real torrent made by `rookparcel-nox create`.

#### D. Detail tabs

- **Mirror:** GTK `detail.rs`, `canvas.rs`, `table.rs`; Win32 `details.rs`, `app.rs`'s tab code; `frontend::detail`.
- **Own:** `detail/`.
- **Done when:** General (Downloaded and Availability bars, Transfer and General groups in a grid), Trackers, Peers, Pieces (the piece map and summary over the list with its block bar), Files (`FILE_COLUMNS`; the D50 filter with "N of M files" and its clear button; priorities, Skip, Prioritize by File Order; Open, Open Containing Folder, Relocate; ⌘A and ⌘F act on it), Speed (the 600 s graph) and Logger work on a running torrent; only the shown tab is asked for (`core.show_tab`); the drawn views follow dark mode and carry accessibility labels; checked on the VM with a real transfer.

#### E. Preferences

- **Mirror:** GTK `preferences.rs`, `sched_grid.rs`; Win32 `preferences.rs` (layout details, help), `help.rs`; `frontend::pref_pages`, `options`, `help`.
- **Own:** `prefs/`.
- **Done when:** every page of `pages()` is built from the declaration (no page drawn by hand), every `Control` kind works, greying follows `Field::enabled`, changes apply at once as described above, a bad value shows its message under the field and is not applied, help shows in tooltips and the help line, Advanced filters and edits, the Scheduler grid paints and drags, `Action::RandomPort` and `ReloadIpFilter` work, the IP filter status shows; Preferences opens at a given page (`prefs::open_at(key)`, used by the status bar); checked on the VM, in German too for text fit.

#### F. Torrent dialogs

- **Mirror:** GTK `add.rs`, `create.rs`, `properties.rs`, `app.rs` (`add_url`, `input`); Win32 `add.rs`, `create.rs`, `properties.rs`, `dialogs.rs`; `frontend::{add, create, properties}`.
- **Own:** `dialogs/`.
- **Done when:** Add New Torrent (an `NSOutlineView` file tree with check boxes and priorities, Save In with recent folders, label, the options and "don't show again", the magnet flow one dialog at a time through `core.next_magnet_dialog`), Add Torrent from URL (prefilled from the clipboard, fetched off the main thread), Create New Torrent (hashing on a thread with progress and cancel, then add or save), Properties (a sheet; several torrents give tri-state fields) and the input dialog all work; checked on the VM with real files and a real magnet from nox.

#### G. macOS integration

- **Mirror:** GTK `macos.rs`, `tray_macos.rs`, `integration.rs` (macOS parts), `closing.rs`, `shutdown.rs`, `notices.rs`, `app.rs` (`start_tray`, `tray_command`, `quit_app`, `notify_completed`, `check_association`, `apply_integration`, `shutdown`, the update functions, `about`); `frontend::{closing, auto_shutdown, integration, legacy, update}`.
- **Own:** `mac/`, `crates/frontend/src/integration.rs` (the moved plist functions and their tests), `crates/frontend/src/legacy.rs` (the `gtk` → `mac` takeover and its tests), `crates/gtk/src/macos.rs` (only to re-export the moved functions).
- **Done when:** on the VM: the quit flow with the closing window (`ROOKPARCEL_DEMO_SLOW_EXIT`), close and minimize to tray, `--minimized`, Dock reopen and Dock menu, the status item's clicks and menu and tooltip, a notification at completion, `open x.torrent` and `open "magnet:..."` with the app running and not running, a second start forwarding its file, the LaunchAgent switched on and off from Preferences, associations set and the startup check, Auto Shutdown's menu and countdown with `ROOKPARCEL_POWER_DRY_RUN=1`, About, Third-Party Notices, the update link with `ROOKPARCEL_UPDATE_URL`, the translocation notice, and a GTK 1.2.5 library taken over; the takeover's tests pass on Windows.

### Wave 2 (after C)

#### H. RSS

- **Mirror:** GTK `rss.rs`, `app.rs` (3311-3600); Win32 `rss.rs`; `frontend::rss`.
- **Own:** `rss.rs`.
- **Done when:** the RSS Downloader window (feeds, filters, history), Add and Edit Feed, the sidebar's Feeds rows with their context menu, and a feed's items in the list (its own columns, double-click downloads) work against a local test feed; checked on the VM.

#### I. Search

- **Mirror:** GTK `search.rs`, `search_helper.rs`; Win32 `search.rs`, `search_helper.rs`; `frontend::search`.
- **Own:** `search.rs`, `search_helper.rs`.
- **Done when:** the search field's mode menu (kept as `ui.search_source`), the Search Results window, the Search Sources window and the helper's get, update, remove and site picker work against a fake Torznab server (the frontend tests' fakes); no third-party binary is downloaded in the agent's run (the real Jackett run is the owner's call).

#### J. Profiles and setup

- **Mirror:** GTK `app.rs` (`create_profile`, `fill_profiles`, `start_profile`, `new_profile`, `profile_folder`, `import_from`, `first_start_guide`), `remote.rs`, `setup_guide.rs`; `frontend::{import, profile, remote, setup_guide}`.
- **Own:** `import.rs`, `profiles.rs`, `remote.rs`, `setup_guide.rs`.
- **Done when:** Import from qBittorrent and µTorrent (with fake client folders), the Profiles menu (start another profile as its own process, New Profile, Open Profile Folder, "Create it?" for a missing one), Connect to Remote (a window before the main window, certificate pinning, the remote title and the lost-connection message) against a local nox, and the Setup Guide (Options and the first start before the engine) work; checked on the VM.

### Wave 3

#### K. Ship

- **Own:** `.github/workflows/{release,ci,macos-gate8}.yml`, `.github/scripts/macos_bundle.py`, `.github/scripts/macos_launcher.c` (removed), `crates/gtk` (macOS code removed: `tray_macos.rs`, `macos.rs`'s `sys`, `HANDLES_OPEN`, macOS paths, the translocation notice; gtk4 under `cfg(all(unix, not(target_os = "macos")))`), `site/build.py` and the site pages, `README.md`, `docs/wiki/`, `docs/RELEASING.md`, `THIRD-PARTY-NOTICES.md`.
- **Done when:** the release workflow builds both architectures, `lipo`s them, bundles, signs ad hoc, starts the app on the runner with a screenshot and makes `Rookparcel-<version>-macos.dmg`; CI no longer installs GTK on macOS; `macos-gate8.yml` checks the native app (and no longer greps for `patorrent-gtk`); `site/build.py` accepts the new dmg name and the home page offers it to every Mac; "macOS 15" becomes "macOS 11" where it describes the GUI; the wiki's install and shortcut pages describe the native app. Merged only after slice L passes.

#### L. Validation

Run by me with one agent, then the owner's hand checks:

- Windows checks; CI green including the MSRV check and the arm64 smoke start.
- VM: the main window in light and dark mode, German and Hebrew; every menu item through osascript; every dialog's snapshot; a real transfer from nox to the GUI on localhost; magnet and `.torrent` opening; second-start forwarding; status item and notification; LaunchAgent and associations; Auto Shutdown dry run; instant-apply Preferences with a refused bad port; the quit flow; the GTK library takeover.
- Scale on the VM with `--demo 100000`: time to the full list, sort, ⌘A, main-thread idle CPU (`top -l`), memory; compared with the Win32 baseline.
- The owner's real Mac: the arm64 slice, Retina, the status icon in light and dark menu bars, Gatekeeper's Open Anyway on a downloaded dmg.

## Not in scope

The Win32-only Web UI host, self-updater and desktop shortcuts; notarization (an Apple developer account, a separate question); status-bar speed menus (in no GUI yet); a monochrome menu-bar rook; moving GTK onto `frontend::app` (BACKLOG).

## Risks

- objc2 and RefCell re-entrancy: B's helper layer, the conventions and a reviewed shell before wave 1.
- One VM for several agents: the lock and Windows type-checks; issue three agents at a time if its queue grows.
- The VM cannot test macOS 11-14, arm64 or Retina: the availability rule, the CI arm64 start, the owner's Mac; an older-macOS VM is in BACKLOG.
- TCC: `UNUserNotificationCenter` may refuse an ad-hoc-signed app (fallback above); the power Apple events may ask for Automation permission at first use.
