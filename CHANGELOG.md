# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-10-26_101801_1793006281: Network/tracker icon decision and revision 4 brief

- Owner decided (D28): network icons use an arrow entering a port/ring; the Trackers tab and all tracker states use a radio tower. Added `assets/ui-icon-explorations/review-2026-10-26/revision3/REGENERATION-BRIEF.md` for the generator: a shared 16 px refresh badge, both new bases with per-state badges, the allowlist change (completed/tracker-working removed), the other 11 redraws and the checks to run. No artwork changed yet.

## 2026-10-26_101510_1793006110: Review of revision 3 bold UI artwork

- Re-inspected all 131 symbols at 64/24/16 px in light, dark and mono, the 24 shipped state icons and 11 toolbar icons at native 16/20/24/32 px, and all-pair mono masks. Confirmed: only the 8 allowlisted pairs are identical; median partial alpha at 16 px is 20 %; most first-review defects are fixed.
- Still to redo (18 symbols): network-ok/no-incoming/offline (plug base reads as a fork), the four tracker states (no shared base; not-contacted unreadable), the 16 px refresh badge, trackers ("(i)" reads as info), allocating, proxy, dht, logger, inactive, reset-columns, add-url, association, all-torrents and download-limit. The first report's runtime-icon count was wrong: 24 in total. Report, sheets and scripts are in `assets/ui-icon-explorations/review-2026-10-26/revision3/`.

## 2026-10-26_100209_1793005329: Generate the full bold UI artwork with native-size proofs

- Owner approved the proposed single-bold, full-set direction (D27). Added editable drawings for all 131 stable IDs, separate larger compound variants, per-size fitted SVGs and 2,358 PNGs across six sizes and light/dark/monochrome. About follows the current Verdigris identity; paused/stopped colours follow activity, and completion remains a check modifier.
- Replaced broken/ambiguous compounds and unrelated shared silhouettes; explicit reuse is limited to eight documented pairs. Updated the gallery with native-size SVG downloads and monochrome review, complete proof sheets, the 24-current-symbol runtime map and a reproducible handoff ZIP. Historical outline/tile explorations remain outside the active manifest.
- Validation: all PNGs decoded; 786 padded monochrome size checks have no overflow/margin failures; all-pair 16/32 px masks have no unexpected duplicates. Full visual review plus independent review prompted final Offline, clock and completed-Stop fixes. Median partial alpha at 16 px is 27% (earlier bold audit: 55%). GTK sizing proof inspected; gallery size/theme/path/filter checks and ZIP integrity passed. Application integration and live platform/DPI checks remain open. No runtime code or existing app selection changed.

## 2026-10-26_093102_1793003462: Review of revision 2 UI icon candidates

- Inspected all 131 symbols × 3 families at 64 px and real 16 px on light/dark, and compared every pair's 32 px mask. 47 symbols need regeneration: malformed drawings (wanted-complete, new/remove-label, language, offline, add-url, rss-rule, forced-state arrows), weak metaphors (about, proxy, bittorrent, rss-history, limits, reset-columns), 19 pixel-identical shape pairs that differ only by colour (queue-up = seeding = upload, error = warning, reannounce = refresh, …), generic tile substitutes, and no pixel-grid fitting (71 % of outline 16 px pixels are semi-transparent). QA.md's earlier pass had missed these.
- Added `assets/ui-icon-explorations/review-2026-10-26/`: report with per-icon fixes, owner decisions and a regeneration spec (pixel-fitted 16 px SVGs, shared badge system, no shared silhouettes), labelled review sheets and their Pillow script. App concept 08 resembles the Edge logo and 03 Ubuntu's mark. No artwork or runtime code changed.

## 2026-10-26_092453_1793003093: Redraw unclear small-size UI icon candidates after full visual audit

- The owner found bad symbols, notably Add Torrent. Format-only validation had missed visual collisions: heavy shared strokes and 75% tile scaling collapsed interior details. Redrew 77 semantic symbols, separated page/action badges, simplified dense geometry, corrected filled bars/dots and made forced/stalled states distinct. Regenerated all 786 SVGs and 4,716 PNGs; tile interior now uses 90% scale.
- Added exact PNG proof to the gallery, a revised-only filter and revisioned asset links; retained stable IDs and shortlist keys. Added paginated 16/24/32 px proof sheets, a before/after comparison, geometry checker and QA notes. No runtime application artwork selection changed.
- Validation: full 16/24 px visual review, independent full 16/32 px audit and follow-up corrections; every PNG decoded for size/ink/alpha; all 786 variants pass padded-canvas clipping checks. Manifest paths/IDs, gallery syntax and force/stall distinction passed. Live browser automation refused the file URL; rendered proof sheets supplied visual evidence. Platform/DPI checks remain pending selection.

## 2026-10-26_084833_1793000913: Broad small-size UI icon exploration batch

- Added `assets/ui-icon-explorations/`: 131 UI symbols in three styles (bold color, strong outline, contrast tile), with light/dark variants: 786 original SVGs and 4,716 transparent PNGs at 16/20/24/32/48/64 px. Covers toolbar, torrent/tracker/network states, actions, navigation, detail tabs, settings, RSS and files.
- Added an offline gallery with actual-size comparison, theme/size controls, search, UI-area filters, favorites and JSON shortlist export; full and excerpted 16 px proof sheets; reproducible export source and manifests. Added a built-in image_gen sheet of 12 new application-mark concepts and its exact prompt. The current application artwork remains selected pending owner choice (gate 10).
- Validation: decoded every PNG and verified dimensions, visible pixels and transparency; checked every manifest path and unique ID; gallery JavaScript syntax passed; visually inspected the 16 px excerpt on both backgrounds. Production integration, full platform/DPI checks and owner selection remain pending. No runtime code changed; application tests were unnecessary.

## 2026-10-23_161421_1792764861: Compact the active plan and changelog; preserve dated Markdown history

### Changed
- Preserved both original files verbatim under `docs/archive/`, with an index and their date/timestamp coverage. The two newest development entries remain below for the current handoff.
- Replaced the accumulated phase history in `PLAN.md` with current status, next steps, all 14 original 1.0 gate numbers, open choices and V2.0/future goals. Moved active decision rationale (D1-D26) to `docs/DECISIONS.md` and unresolved detailed follow-ups to topic sections in `docs/BACKLOG.md`; reconciled completed work that older notes still described as missing.
- Added repository instructions to keep root documents around 16 KiB / 200 lines, rotate complete older entries into uniquely named Markdown archives, and read history only when needed. Markdown keeps history searchable and diffable without decompression.

### Verified
- Compared the snapshots with the pre-cleanup files, checked decision IDs and retained entry content, and reviewed the active plan/backlog for unresolved work. Checked active-document local links and Git whitespace. Documentation only; application tests were not needed.

## 2026-10-23_155639_1792763799: The Win32 dialogs use the system's message font (Segoe UI 9 pt), as the main window and 2.2.1 do

The owner's go-ahead on the previous entry's finding.

### Issues
- Every dialog template named `MS Shell Dlg` at 8 pt, which the dialog manager resolves to Tahoma 8 pt (a 6 x 13 base) on Windows 11, while the main window takes the message font from `NONCLIENTMETRICS` (Segoe UI 9 pt, 7 x 15) and µTorrent 2.2.1's dialogs measure as the same. The dialogs looked smaller and older than the window around them.

### Fixed
- `dialogs.rs`: `Template::new` writes `DS_SETFONT` with `message_font()`: the face and point size of `lfMessageFont` from `SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS, .., 96)` (a negative `lfHeight` is the character height in pixels at 96 DPI, so points = height x 72 / 96; the dialog manager scales the font to the window's DPI as before). `MS Shell Dlg` 8 pt stays as the fallback when the call fails. Every dialog (Add New Torrent, Preferences and its pages, Create, Properties, RSS, Remote, Auto Shutdown, the notices, the input boxes) keeps its unit layout and grows by 7/6 across and 15/13 down.
- A test, `dialogs::tests::templates_use_the_system_message_font`: the face is not `MS Shell Dlg`, the size 8 to 12 pt, and a dialog built from a template answers `WM_GETFONT` with that face, its button with the same handle.

### Verified
- `cargo test -p patorrent --bin patorrent`: 59 passed (no pixel-pinned test needed a change: the layouts are in dialog units and the Add dialog's tests measure against the window); `cargo clippy -p patorrent --all-targets -D warnings` clean; `cargo fmt`.
- Seen by eye in a portable copy of the debug build (`--demo 3`): Add New Torrent is 751 x 602 px at 100 % now (646 x 527 in Tahoma), Preferences 770 x 547, Create New Torrent 541 x 635, every text in Segoe UI 9 like the main window, nothing clipped on the three; not every dialog was opened.

## 2026-10-23_154951_1792763391: The Win32 Add New Torrent dialog on 2.2.1's one-column layout, resizable, with "Don't show this again"

The owner found the dialog still clunky and put a portable µTorrent 2.2.1 in `z_exclude_uTorrent/` (gitignored now) to compare with. Computer use refuses µTorrent by policy, so the comparison went through scripts: `PrintWindow` captures of 2.2.1's main window and of its Add dialog (opened with the CachyOS torrent), and a dump of every child control's class, id, rect and style.

### Issues
- Ours was µTorrent 3.x's two-column dialog (Save In, Name and the options at the left, Torrent Contents at the right), not 2.2.1's. 2.2.1 is one column: a "Save As" box with the full path (folder and name in one field) and the four options in two columns under it; Torrent Contents under that with Name / Comment / Size / Date, Select All and Select None at the right of the Date line and the file list across the whole width; then Advanced..., OK, Cancel and a "Don't show again" box. And it resizes (the owner's copy was stretched to 1113 x 650). In the two-column dialog the list was 209 units wide, so file names ended in "..." (the owner's screenshot), and the fixed size could not help.
- 2.2.1's dialog measures as Segoe UI 9 pt (its buttons 88 x 26 px at 100 %: a 7 x 15 base); ours use `MS Shell Dlg` (Tahoma 8 pt, 6 x 13), which is also why the previous entry's 420 units came to 646 px. Left as it is (Known TODOs "Win32 dialog font").

### Decided
- A hybrid, the owner having left the choice here: 2.2.1's one-column structure and resizing (the list gets the whole width and grows with the window, which is what makes the file names readable), 3.x's separate Save In and Name fields (a rename and a folder choice are clearer apart than as one path to split). "Don't show this again" is in; "Advanced..." (2.2.1 opened the per-torrent properties before adding) is not, there being no properties-before-add yet.

### Fixed
- `crates/win32/src/add.rs`: the template is 420 x 300 units: a Save In box (the folder combo and "...", a Name: line, then Skip hash check / Label: and Start torrent / Add to top of queue in two columns at 2.2.1's spacing), a Torrent Contents box (the four lines, Select All / None on the Date line, the list 392 wide under them), "Don't show this again" at the bottom left and OK / Cancel at the right. The group boxes have ids now (`ID_BOX_SAVE`, `ID_BOX_CONTENTS`) so they can be anchored. The dialog gets `WS_THICKFRAME | WS_MAXIMIZEBOX` after `fit_texts` (`resizable`), the template's size being the smallest (`WM_GETMINMAXINFO`); `layout` moves and grows the controls on `WM_SIZE` from their rects at the template's size by an anchor table (`ANCHORS`: the list and the contents box grow both ways, the combos, the name and the values with the width; "..." and Select All / None move with the width, OK / Cancel with both, the check box with the height; a combo's height is its open one from `CB_GETDROPPEDCONTROLRECT`, as `GetWindowRect` gives the closed one). The list's Name column takes what Size (50 units) and Priority (48) leave (`fit_columns`, again when a folder opens or closes, as the scroll bar may come or go). The size the dialog closes with (`WINDOWPLACEMENT`'s normal rect, in 96-DPI pixels) comes back in `Shown::window`; `Setup::window` opens the next dialog at it (never below the template's, centred where the template was, kept on the monitor's work area).
- `show` returns `Shown { choice, window, dont_show_again }`; `add::remember(prefs, &shown)` keeps the size (`add.window`; new `add::window_size` / `remember_window` in the frontend) and turns `ui.show_add_dialog` off after "Don't show this again" on OK. Both callers in `app.rs` (a `.torrent`, a magnet's metadata) call it and save the preferences, on Cancel too.
- Strings: `AddName` is "&Name:" now (a label on a line, not a box title; the 34 tables got their colon, French its " :" and Chinese its full-width one); new `AddDontShowAgain` ("Don't show this again", 2.2.1's text) in all 34 tables, machine translations like the rest; `en.lng` regenerated. `AddOptions` ("Torrent Options") is no longer used by the Win32 dialog (GTK's still has the box).
- Tests: `the_list_and_the_buttons_follow_the_window_size` (the sizing border, the minimum, every anchor kind after a 100 x 80 growth, the drop-down's open height, the Name column filling the list), `the_size_is_remembered` (a remembered 900 x 700 opens at it, 100 x 100 is lifted to the template's, `remember` stores the size and the preference), `ok_notes_dont_show_again`; `shows_the_torrent_and_its_files` checks the new box.
- `.gitignore`: `/z_exclude_uTorrent` (the owner's portable 2.2.1, the reference for the Win32 dialogs).

### Verified
- `cargo test -p patorrent --bin patorrent add::`: 11 passed; `cargo test -p patorrent-frontend lang::`: 12 passed (`en.lng` regenerated with `PATORRENT_WRITE_LNG=1`; the shipped tables are built from it, so they were one key short until then); `cargo clippy -p patorrent -p patorrent-frontend --all-targets -D warnings` clean; `cargo fmt`.
- Seen by eye: a portable copy of the debug build (`--demo 3`, a 1.41 TB multi-file torrent): the dialog is 646 x 527 px at 100 % with the whole width for the file names (no "..."), the scroll bar inside the Name column's reckoning (no horizontal bar); grown by 300 x 160 by script, the list and the contents box grew with it, the combos and the name line too, Select All / None, OK and Cancel moved; Cancel wrote `add.window=946,687` to `patorrent.ini`.
- Not run: the GTK side is unchanged but for `Setup::window`, which it ignores (type-checked on CI). µTorrent was started and ended by script (its Add dialog would not take a posted close); nothing was added to it.
