# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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

## 2026-11-06_092025_1793953225: Website: favicon links carry the icon's hash

- The owner still saw the old icon as the site's favicon. The deployed `img/icon-*.png` were already the Postal Rook (byte for byte); browsers keep a favicon per page and do not refetch a changed file at the same URL. `site/build.py` now adds `?v=<hash of the icon PNGs>` to every page's `rel="icon"` and `apple-touch-icon` links, so a new icon gets a new URL. Test in `WholeBuild`; WEBSITE.md notes it.

## 2026-11-06_085947_1793951987: Release 1.2.5 published, the first as Rookparcel

- The Release workflow built all 14 files; the owner published the draft, and it is GitHub's Latest. The news entry is on `main`. Live site checked: `update.txt` says `stable=1.2.5`, the download page lists only `rookparcel-1.2.5` / `Rookparcel-1.2.5` files, and "PaTorrent" remains only in the news text about the rename.

## 2026-11-06_084738_1793951258: Wiki screenshots retaken; v1.2.5 tagged

- `docs/wiki/images` (18 changed, same sizes) and `docs/screenshots/search-win-dropdown*.png` retaken from the 1.2.4 demo on the hidden desktop: Rookparcel title bars and icon, `D:\Downloads\rookparcel-mock`; pushed to the wiki. The other search-guide shots never showed the name and need a real Jackett, so they stay.
- CI, FreeBSD and Fuzz green on `50362f50`; `v1.2.5` tagged there and pushed, so the Release workflow builds the draft. The news entry waits on the local branch `news-1.2.5`.

## 2026-11-06_082750_1793950070: Version 1.2.5: CI green again; release 1.2.5 prepared (1.2.1-1.2.5)

- **Fixed:** CI on `main` failed since the rename push: the runners' clippy 1.99 flags `needless_borrows_for_generic_args` on `.filter(&absolute)` in `crates/frontend/src/legacy.rs`, which stopped the workspace's clippy at the frontend crate. Local clippy (1.90) does not have the lint; workspace clippy and the `legacy` tests pass here.
- **Release 1.2.5**, the first as Rookparcel: [notes](docs/releases/1.2.5.md) (the rename, the Postal Rook, the takeover and how PaTorrent users move over, the 1.2.1 login fix), the Flatpak metainfo's `<release>`. GitHub's release titles up to 1.2.0 now read "Rookparcel X (released as PaTorrent)"; their notes and file names are unchanged.

## 2026-11-06_082258_1793949778: Website: screenshots of Rookparcel 1.2.4

- `site/img/screenshot-windows.png` (1000x680) and `add-new-torrent.png` (751x602, also `docs/screenshots/`) are new shots of the 1.2.4 Win32 GUI: "Rookparcel 1.2.4" in the title bar (built with `ROOKPARCEL_BUILD` empty), the Postal Rook icon, the toolbar search box, a downloading torrent's General tab, and the current Add New Torrent layout with the Sintel torrent. Taken on the hidden desktop from a portable demo copy (`--demo 40`), with `TMP` and `dir.download` pointed at `D:\Downloads` so no user folder shows. `index.html`: the dialog's size and alt text follow the new layout.
- Checked: `build.py --strict`, the site tests, and the home page in the browser pane.
- Still "PaTorrent" on GitHub: the release titles and bodies up to 1.2.0, and the 1.2.0 file names on the download page (the files are named so; `build.py` accepts both prefixes). Both go once a Rookparcel release is published. The wiki's other screenshots still show PaTorrent title bars.

## 2026-11-05_230706_1793916426: Published: the rename on GitHub `main` and the wiki

- The owner renamed the repository to `Balls0fSteel/Rookparcel`; Pages serves `https://balls0fsteel.github.io/Rookparcel/`. `origin` now names the new URL. `main` was fast-forwarded to `58e5b1fa` (1.2.2-1.2.4, the icons and the site colours) and pushed.
- Wiki `b97aafa`: `docs/wiki` and the search guide copied over; `Installing-`, `Updating-`, `Where-...-keeps-its-files` and `Running-...-nox` have new names, and the old pages are one-line pointers to them (they also show in GitHub's automatic page list). Checked first that the wiki had no edits outside `docs/wiki`. The screenshots still show the old name in title bars.

## 2026-11-05_230319_1793916199: Version 1.2.4: the small app icon is the courier rook (09, redrawn)

- The owner chose GPT's `09 Compact Courier` for 16-32 px. Redrawn in `assets/rookparcel-icons`: `src/courier-32.svg` (20-32 px, vector) and `src/courier-16.txt`/`.svg` (16 px, pixel by pixel), with a heavier, long two-toned rook beak (pale bare base), a smaller lidded eye, a peaked cap with band and gold badge, and the flat cream square replaced by a paper tile with a stamp-red rim that holds its edge on dark taskbars. The stamp stays from 48 px; `build.cjs` rebuilt `04-postal-rook`. D60 updated.
- Checked: the Win32 and nox builds, `the_app_icon_at_every_size`, `comparison.png` (before, GPT's 09, now; light and dark), and the icons Windows extracts from the built `rookparcel.exe`.

## 2026-11-05_225249_1793915569: Postal Rook candidates designed for small sizes

- Saved 11 built-in image-generation masters, exact prompts, PNG frames and ICOs in [the small-icon study](docs/art/postal-rook-small-2026-11-05/README.md). Decoded all 110 ICO frames; reviewed actual 16/20/24/32/48 px on light and dark backgrounds against the original stamp.
- Independent visual review favors 10 Night Courier, 11 Closed Envelope and 09 Compact Courier. These are new candidate studies; no production assets, D60 decision or program version changed. Next: owner selection and dedicated small-size refinement.

## 2026-11-05_225036_1793915436: Website in the Postal Rook's colours

- `site/css/style.css`: the teal and copper palette (from the Verdigris icon) becomes the stamp's: a navy hero and page heads with a stamp-red rule, red buttons, a paper-cream button with a kraft rim on the hero, gold hero links and focus ring, a pale red list hover, our benchmark bars in red; light and dark. Links stay a navy blue (red reads as an error). `--copper` is now `--accent`; every page's `theme-color` is `#14233d`.
- Checked: every text pair at WCAG AA or better in both schemes (lowest 5.7:1, the buttons); `build.py --strict` with the test releases, the site tests, and the pages in the browser pane in light and dark (computed colours, the benchmark bars, no console errors).

## 2026-11-05_224301_1793914981: Version 1.2.3: the Postal Rook is the app icon (D60)

- Family `04-postal-rook` (`assets/patina-icons/exports/app/`, built by `assets/rookparcel-icons/build.cjs`): the drawn 16 px and 20-32 px marks, and from 48 px the generated stamp the owner chose. `app-icon.txt` and the Flatpak `ICON_DIR` name it, so the Win32 and nox executables, GTK, the Web UI favicon, AppImages, macOS bundles, the Flatpak, the installer and the site all follow; D57's Verdigris is superseded.
- Checked: the Win32 and nox builds, `the_app_icon_at_every_size`, the site tests; the icons Windows extracts from `rookparcel.exe` (16-32 px drawn, 48 and 256 px the stamp) and a hidden demo window's title bar. The site's teal and copper colours still follow the old icon.

## 2026-11-05_223827_1793914707: Postal Rook app icon redrawn as vectors, per size

- `assets/rookparcel-icons/`: the owner's chosen Postal Rook as hand-written SVG in three drawings: a pixel-by-pixel 16 px mark, a 20-32 px head close-up (cap, pale rook face, beak on a kraft parcel) and the 48-512 px stamp (walking rook, courier bag). `build.cjs` makes the PNGs and a 10-frame ICO (25 KB against the cropped candidate's 145 KB); `comparison.png` sets its real frames beside the generated candidate's. At 16-32 px the head, cap and parcel stay readable where the crops blur. Candidate only: not yet wired into the programs.

## 2026-11-05_222944_1793914184: Cropped Rookparcel ICO candidates

- Exported 25 ICO files: light/dark crops of all 12 concepts plus a transparent Postal Rook stamp; matching PNGs, preview, crop coordinates, exporter and ZIP in the artwork folder. Postal Rook is the owner's favorite.
- Decoded all 250 embedded frames (10 sizes, 16-256 px) and visually checked the crop preview. Small sizes are resampled studies; dedicated pixel refinement and production integration remain open. No program change or version bump.

## 2026-11-05_221629_1793913389: Rookparcel UI icon studies

- Saved four concept sheets (96 labeled studies), exact prompts and visual review in [the UI artwork index](docs/art/rookparcel-ui-2026-11-05/README.md). Reviewed existing toolbar and small-size proofs with an independent audit.
- Verified all four PNGs decode; documented generated semantic inconsistencies. Exploration only: no production assets or program version changed; native-size vector refinement remains open.
