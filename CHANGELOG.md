# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-10-28_134318_1793194998: 1.1.0 published

- **Published** on the owner's go-ahead at 13:42 UTC, marked Latest: [release v1.1.0](https://github.com/Balls0fSteel/PaTorrent/releases/tag/v1.1.0) on `65a3d0ca`. The files were not hand-tried before publishing. Pages runs 40698837791 and 40698837651 passed; the site's `update.txt` says `stable=1.1.0`. CI run 40695861609 on `65a3d0ca` passed, macOS included, with the external-address test fix.

## 2026-10-28_132134_1793193694: Flaky macOS external-address test fixed

- **Cause:** `crates/core/tests/external_ip.rs`'s alert helper dropped the rest of a popped batch when its wait matched. On a slow runner `ExternalIp` came in the same batch as `TorrentAdded` and was lost (CI run 40645656823; the timed log showed the tracker had answered). The other integration tests already keep a backlog; this file now does too (thread-local, one session per test thread). The "scrape failed: banned by IP filter" in that log is the expected SSRF rule for a loopback `/scrape`.
- **Validation:** the file's three tests pass five runs in a row on Windows; core clippy and fmt clean. Test-only, so no version bump.
- **Release 1.1.0 retagged:** the first `v1.1.0` tag run failed on the old installer check (fixed in `a055a3c8`; a `workflow_dispatch` rerun builds but skips the tag-only release job). Nothing had been published, so the owner had the tag moved to `65a3d0ca`; Release run 40695889139 passed every job and opened the draft release (14 files, 126 MB), not yet published.

## 2026-10-28_113636_1793183796: Release 1.1.0 prepared

- **What is in it since 1.0.1:** V1.1's search box and importers (D35, D36), every security fix from the review (SEC-01 to SEC-26; remote protocol 9), the Win32 hand-test fixes and per-build version strings (1.0.2–1.0.3, D37), and the navigation and menu icons with artwork revision 8 (1.0.4–1.0.8). Release text: [docs/releases/1.1.0.md](docs/releases/1.1.0.md).
- **Version 1.1.0** (owner's choice over 1.0.8, as it carries V1.1); peer ID `-PA1011-`, since the security fixes changed the engine's wire behaviour (D37). The Flatpak metainfo's first `<release>` is 1.1.0; the README names it.

## 2026-10-28_112815_1793183295: Revision 8 reviewed; three refinements (1.0.8)

- **Review:** an independent reviewer passed all eight revision 8 symbols, with evidence sheets and a [report](assets/ui-icon-explorations/review-2026-10-28/revision8/REPORT.md). Applied from its optional notes: the notices C has a 4-row counter (it read as a heavy square at 20–32 px), the import bracket matches log out's height and is centred on the arrow, and the edit eraser gap is one clear pixel. Not applied: a grey × on the orange feed, as the renderer gives an icon one colour. It also corrected the README: four of the eight are vector paths, not pixel art.
- **Validation:** rebuilt and re-audited (zero overflow, margin failures and unexpected masks; no new near matches); the frontend embed tests pass.

## 2026-10-28_111544_1793182544: Artwork revision 8, the last missing menu icons (1.0.7)

- **Change:** eight symbols drawn in the house pixel art (website, check-updates, notices, import, window, edit, feed-disabled, no-label; [artwork README](assets/ui-icon-explorations/README.md)), so every menu command with a fitting meaning has an icon: Help's Website / Check for Updates / Notices, File > Import, the tray's Show / Hide, Edit Feed, Disable / Enable Feed (by state, through the item tag), the Add dialog's Check / Uncheck (select-all / clear-selection), and No Label in both GUIs' category lists. Nothing was copied from other icon sets.
- **Validation:** build and geometry audit clean (139 icons, no new near matches, existing exports byte-identical); frontend, Win32 tests and clippy, GTK clippy and fmt pass; the Win32 demo shows the Help and File menus and No Label at 100 %.

## 2026-10-28_110102_1793181662: GTK navigation icons (1.0.6)

- **Change:** the GTK sidebar, detail tabs and Preferences page list show the shared `UiIcon` symbols, following the light/dark theme like the toolbar (one registry for both). Preferences' `StackSidebar`, which cannot show icons, is now a `ListBox` kept in step with the stack; tab labels keep their mnemonics through a handler of their own. GTK menus stay text only (GTK 4 popover menus draw icons only in button rows).
- **Validation:** `patorrent-gtk` type-checks and passes clippy (`--all-targets -D warnings`) through the stand-in pkg-config; fmt passes. Not run: no GTK display here, so layout, mnemonics and the dark palette wait for a Linux check (BACKLOG).

## 2026-10-28_091820_1793175500: Win32 menu icons (1.0.5)

- **Change:** the main menu, the torrent, tray, files, feed, feed-item and column menus, and the Add and RSS dialogs' menus show the bold symbols beside their commands (`UiIcon` gains 33 menu symbols; `bgra` and `PRIORITY_ICONS` in `patorrent_frontend::icons`). Win32 maps command IDs to icons in one table and applies them when a menu opens, as 32 bpp premultiplied bitmaps cached per size; greyed items get the grey copy. Check-marked items keep only their mark.
- **Validation:** frontend (194 + 4) and Win32 (67) tests, clippy and fmt pass; the GTK crate type-checks. A portable `--demo 40` run, menus opened by window messages, shows File, Options and the torrent menu with upright, cleanly blended icons beside the check marks at 100 %.

## 2026-10-28_091009_1793175009: Win32 navigation icons (1.0.4)

- **Change:** the category list, the detail tabs and the Preferences page tree draw the bold artwork's symbols: `UiIcon` (24, at 16/20/24/32 px in both palettes) with `category_icon`, `tab_icon` and `page_icon` in `patorrent_frontend::icons`, so GTK can use the same table. The state-icon macro now serves both enums. The main window rebuilds its list with the other images on a DPI change; Preferences makes its own at the dialog's DPI.
- **Correction:** the audit below counted 37 embedded symbols; it was 35 (24 states, not 26).
- **Validation:** frontend (194 + 4) and Win32 (67) tests, clippy and fmt pass; the GTK crate type-checks with the stand-in pkg-config. A portable `--demo 40` run shows every tree entry, tab and Preferences page with its symbol at 100 %. Not checked: higher DPI, RTL.

## 2026-10-28_090116_1793174476: Icon coverage audit

- 37 of the 131 bold symbols are embedded (toolbar and states). The category list, detail tabs, Preferences pages and menus show no icons, though about 60 of their symbols are already drawn. Nine are not drawn; each has MIT/ISC references (Pixelarticons, Lucide, Fluent) to redraw on the 16 px grid. Mapping and toolkit limits: [BACKLOG](docs/BACKLOG.md#ui-icon-coverage-audit-2026-10-28_090116_1793174476).

## 2026-10-28_083421_1793172861: Blank rows found and fixed: the list painted outside its paint cycle (1.0.3)

- **Reported:** on the 1.0.2 test build the whole torrent list went blank after Select All (the owner); replayed on the `--demo` mock with a scripted click, Ctrl+A, Ctrl+T and mouse hover (a scratch PowerShell script taking screenshots): the hovered row lost every cell and its Done bar, and stayed so until a click repainted it.
- **Cause:** besides the real paints, the list view sends the custom-draw item stages with an empty `rc` and another device context while it measures a truncated name for its in-place tooltip (`LVS_EX_LABELTIP`); a trace showed `rc (0,0,0,0)` on a different `hdc` for the hovered row. `selection_pre_paint` filled the row's rectangle into that context, which is the window's own, outside any paint: a bare bar with no text. Before this morning only selected rows were painted that way (the owner's original "selected row shows no text"); the alternate-row shading extended it to every row, so hovering blanked them one by one.
- **Fix:** `controls::is_paint` (a non-empty `rc`) gates the selection bar, the alternate shading, the focus rectangle, the Done bar and the Blocks bar. The borrowed-state invalidation from the morning stays as insurance; it was not the cause.
- **Validation:** the replay shows every row intact while hovered after Select All and Stop; Win32 tests (67), clippy and fmt pass. Version 1.0.3.

## 2026-10-28_075806_1793170686: Owner hand-test findings fixed; version 1.0.2 and per-build version strings (D37)

- **Stopped rows showed a speed:** a torrent's traffic statistics tick only while it runs, so a stopped torrent reported its last rate for ever ("1009 kB/s" in Down Speed). `stop_torrent` now resets the rates (`Stat::reset_rates`, totals kept), and the Down/Up Speed cells are empty for any paused or queued torrent. Tests: `reset_rates_keeps_the_totals`, `stopped_rows_show_no_speed`.
- **Blank rows:** the list's cell text comes from `with_app`, which yields nothing while the app state is borrowed (a message box or a control updated inside the borrow makes the list paint). A double-buffered list kept such blank cells until the row's next repaint, which an idle row may never get. The `LVN_GETDISPINFO` handler now invalidates the list when the borrow fails, so the row paints again as soon as the state is free. The caller that held the borrow during the owner's add was not identified (BACKLOG).
- **Scheduler grid:** painted into a memory bitmap and copied in one `BitBlt`, with `WM_ERASEBKGND` swallowed, so dragging over cells no longer flickers. Colours follow µTorrent's manual (dark green `008000`, light green `90ee90`, white, light red `ffa0a0`; Seeding only was blue) with black grid lines; disabled, the table is drawn as 2.2.1 draws it (white cells, grey legend swatches 0x38 / 0xa2 / 0xd4, grey text, from the owner's Scheduler screenshot), and `WM_ENABLE` repaints. The GTK grid shares the colours (fades when greyed, unchanged).
- **Alternate row shading:** `ui.alternate_rows` (`GuiOptions::alternate_rows`, on by default like 2.2.1's "Alternate list background color"), a box in Preferences > UI Settings (Win32 and the shared `pref_pages`, gated by `Features::alternate_rows`; GTK passes false). Win32 paints every odd unselected row with the window colour at 15/16 through `controls::selection_pre_paint`, so the torrent list, the feed view and every detail-tab list get it; Apply repaints. New string `PrefAlternateRows` in the 35 tables (34 machine translations).
- **Hotkeys (Win32):** Ctrl+S Start, Ctrl+T Stop, Ctrl+Shift+P Pause and Enter (torrent list focused) for Properties, beside the existing Ctrl+O/U/N/P/A/F, Del and Shift+Del; the context menu shows Del, Shift+Del, Ctrl+S/T/Shift+P and Enter, the File and Options menus Ctrl+O/U/N/P (`Hotkey` labels from new strings `KeyShift`, `KeyDel`, `KeyEnter`, translated where Windows labels the keys: Strg/Umschalt/Entf/Eingabe, Maj/Suppr/Entrée, ...). Ctrl+Space stays the list view's own (toggle the focused row's selection). F2: nothing to rename on Win32. In the feed view Enter downloads the selected items, as double-click does.
- **Version 1.0.2, bumped with every change set (D37):** `Cargo.toml` and the Windows manifest; the rule is in AGENTS.md and RELEASING.md step 1. `PEER_ID_PREFIX` stays `-PA1010-` as a counter (`peer_id_prefix_is_a_pa_counter` replaces the version-tracking test). A build that is not a tagged release shows `1.0.2-dev.<commit>` in the title bar, the tray tooltip, About, the remote titles, the Web UI's version and `--version`: `patorrent_frontend::BUILD` / `NAME_BUILD`, computed by the frontend's build script from git (or `PATORRENT_BUILD`, which `build.bat` sets with `.dirty` for uncommitted changes); the user agent, the peer ID, the update check and the remote protocol keep the plain version.
- **Icons, revision 7** (`assets/ui-icon-explorations`): the toolbar's Add Torrent / Create (a shared pixel-art page with a dog-ear, one drawing for every size: the 24 px variant had mixed 2 and 3 px edges), Remove (a trash can centred on x 8 so lid, ribs and slots keep one width at 24 px) and Add from URL were redrawn with uniform strokes. **Add from URL is a horseshoe magnet with the plus badge again** (upright, equal 2 px arms, the revision 4 review's own alternative): the owner's finding called the icon a magnet, but D29 had chosen a chain link after two magnets failed review, decided for the magnet 2026-10-28_081814_1793171894 (D38; the chain link stays in git at `16b14bf0`). The four checking states are lens-based: a magnifier with a check (checking files), with "…" (checking resume data), and a list with a small lens or "…" for the queued pair; the floppy and the 1 px clock are gone. `recheck` shares the lens. 9 ids × 36 outputs changed, `check-geometry` clean, the frontend icon tests pass (9).
- **Validation:** workspace build, core (774), frontend (193 + 4), Win32 (67) and nox (146) tests pass on Windows; workspace clippy with warnings denied and fmt are clean; `patorrent-gtk` lints clean for `x86_64-unknown-linux-gnu` with the stand-in pkg-config (compile check only). Nothing was clicked: the owner's re-test list is in the backlog.
