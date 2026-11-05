# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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

## 2026-11-05_220518_1793912718: Rookparcel artwork exploration

- Saved 12 PNG concept sheets covering rook, parcel, bird, monogram and mascot directions, with exact prompts in [the artwork index](docs/art/rookparcel-candidates-2026-11-05/README.md).
- Exploration only: no identity selected or production icon changed. Verified all 12 PNG files and the prompt index are present; no build or tests needed for these assets. Next: select and refine a candidate.

## 2026-11-05_220414_1793912654: Version 1.2.2: PaTorrent is renamed Rookparcel (D59)

Entries below this one keep the old name PaTorrent.

- **Renamed** everywhere outside history (`docs/archive`, `docs/releases`, older entries here, the icon art): crates `rookparcel-*`, binaries `rookparcel(.exe)`, `rookparcel-nox`, `rookparcel-gtk`, app id `io.github.balls0fsteel.Rookparcel`, packaging and contrib files, wiki page names, `ROOKPARCEL_*` variables, the 35 language files (Korean particles adjusted for a consonant-final name; the About text loses the PatinaTorrent long name). Peer ID `-RP1014-` (`PA` stays in the client table as PaTorrent), User-Agent and BEP 10 `v` `Rookparcel/…`, remote protocol 16, the ping probe still 32 bytes.
- **Takeover of a PaTorrent install** (`crates/frontend/src/legacy.rs`): the old default roots (`%APPDATA%\PaTorrent`, XDG `patorrent`, Application Support) are renamed once at start, with `patorrent.ini` renamed in every settings folder and paths into the old root rewritten; a portable `patorrent.ini` is taken over in place. A running PaTorrent blocks it: `lockfile` holds `patorrent.lock` too, and the start says the folder is in use. The registry's ProgIDs, Default apps entry and per-profile `Run` values (`integration::adopt_patorrent`), GTK's desktop and autostart entries and macOS's LaunchAgent move to the new names; the setup removes an installed PaTorrent; `service install` replaces a `PaTorrentNox` service and moves `%ProgramData%\PaTorrent`.
- **Still read:** the `patorrent resume db` tag, `PATORRENT_LANG`/`_BUILD`/`_NO_UPDATE_CHECK`/`_STORE`, `X-Requested-With: PaTorrent`, `patorrent-search:` URLs, a PaTorrent GUI's remote upgrade (answered with the version); the DPAPI entropy keeps its bytes. The site lists old- and new-named release assets. No update bridge (owner): 1.2.x installs are not told.
- Docs: D59, README, DAEMON.md "Coming from PaTorrent", RELEASING.md's first-release steps (repository rename, wiki push, registering `RP`).
- **Upgrade checked on Windows** (scratch `APPDATA`, real 1.2.1 `patorrent-nox` from `cdb0578d`, then 1.2.2's): 16/16 checks. A running PaTorrent makes Rookparcel exit 1 with nothing moved; afterwards the folder moves, `patorrent.ini` is renamed with its paths following, no old lock is left, both the default and a `work` profile's torrents come back, a second start moves nothing, and a portable folder is taken over in place without touching `APPDATA`. Its library was in the journal only, so the old `resume.db` tag rests on the unit test. Not run: the GUIs on a real install (the registry takeover has a scratch-key test), the setup over an installed PaTorrent, Linux, macOS.

## 2026-11-05_205340_1793908420: Version 1.2.1: a login banned meanwhile is answered "banned"

- **Fixed:** `check_login` (`crates/nox/src/webui/api.rs`) returns `Login::{Ok, Failed, Banned}`. A login refused because another guess got the address banned meanwhile is answered 403 with the ban text instead of "Fails."; a remote-session login in that case gets 403 instead of 401 (the client already shows 403's text). `login`'s separate ban check before it is gone, so the check and the count happen under one lock.
- Found by macOS CI (`parallel_guesses_stop_at_the_ban_limit` counted 6 "Fails."); the ban itself always held. Clippy (nox, Win32), all nox tests on one thread at Idle priority, and that test 10 times in a row pass. BACKLOG item removed.

## 2026-11-05_183712_1793900232: Release 1.2.0 (1.1.41-1.1.78)

- **1.2.0** ships 1.1.41-1.1.78 (D58): profiles and portable mode everywhere (D42), the Files filter (D50), the search helper (D51), the Setup Guide (D53), "Use only this network" (D54), help texts (D55), the 2026-11-05 bug hunt (37 of 38 fixed, one ruled out) and the backlog fixes. Release notes: [docs/releases/1.2.0.md](docs/releases/1.2.0.md); Flatpak metainfo `<release>` added.
- Version bumped to 1.2.0. Peer ID `-PA1013-` (owner, 2026-11-05_183712_1793900232) for the wire changes of 1.1.66, 1.1.70, 1.1.74 and 1.1.76. The remote protocol is 15 (14 since 1.1.58, 15 since 1.1.70).
- **Published 2026-11-05_204405_1793907845:** CI, FreeBSD and Fuzz green on 8e613e30 (one fuzz job hung in the runner's `apt-get` and was rerun); the draft's 14 files built, checksums and `patorrent-nox --version` checked, the text matches the notes. The website (FAQ and download text from `site-profiles`, the `site/news.html` entry) went out right after; `update.txt` says `stable=1.2.0`.
- **Issue:** macOS CI on the website push failed `parallel_guesses_stop_at_the_ban_limit`: a guess banned between `login`'s ban check and `check_login`'s is refused but answered "Fails.". The ban holds; logged in BACKLOG "Validation" with the fix.

## 2026-11-05_152743_1793888863: Version 1.1.78: search hash check, folder case, Web UI private flag and queue order, RSS read marks, unknown sizes, help texts

- **Fixed:** a merged search result whose fetched torrent has another info-hash is refused (new string `SearchErrHashMismatch`, all tables).
- **Fixed:** `same_dir` ignores case only on Windows and macOS, so `/Data` and `/data` stay apart on Linux (frontend and nox).
- **Web UI shim:** restored torrents' `isPrivate` fills in (metadata at up to 128 per tick); `queuedDL` sorts as libtorrent orders it; RSS read marks persist (`useRegex` still ignored); an unknown size is an empty cell, not 0 B.
- Serbian mnemonic fixed; help texts corrected (Hashing threads says it is not used); PORTING.md rows added.
- Clippy, frontend (288) and nox tests pass, one thread at Idle priority. BACKLOG drops the items closed in 1.1.75-1.1.78.
- **Validated (2026-11-05_152743_1793888863):** on the Xubuntu VM, `patorrent-gtk` passes stable clippy and its 31 tests, so the GTK changes of 1.1.75 and 1.1.77 build on Linux (not run in a GTK session). PLAN trimmed under 16 KiB.
- **CI fix (test only, no bump):** macOS CI failed `a_body_streams_to_the_sink` with `TimedOut`: its 12-byte body could not earn time against 1.1.76's `MIN_STREAM_RATE`, so it passed only if the whole body came within the 1.5 s grace. It now sends three 2 KiB pieces 1 s apart (each earns 2 s), and checks again that the body outlasts one timeout.

## 2026-11-05_152150_1793888510: Version 1.1.77: update folder, update URL overrides, tray Show, private DHT/PEX, "Restoring torrents...", scrapes logged

- **Fixed:** Win32 self-update downloads its setup into `%LOCALAPPDATA%\PaTorrent\update` instead of a shareable `%TEMP%\PaTorrent-update` (the old folder is emptied and removed).
- **Fixed:** `PATORRENT_UPDATE_URL` / `PATORRENT_UPDATE_FILES` are honoured only for https or plain http to this machine (`override_allowed`); otherwise the real URLs are used.
- **Win32:** the tray menu says Show for a minimized window; the status bar shows "Restoring torrents..." (new string `BarRestoring`, all tables) until the session's torrents are restored.
- **Win32, GTK:** Torrent Properties greys DHT and PEX when every selected torrent is private and does not apply them then; scrape replies and failures go to the Logger like other tracker alerts.
- Validated: clippy, the frontend tests (284), and the Win32 tests in about 280 parallel runs of this tree; GTK not built on Linux yet.
- **Investigated:** the Win32 test binary's heap corruption under parallel load is real (full page heap caught a verifier stop once in ~148 runs) but not traced to code yet; BACKLOG has the evidence and two parallel-test flakes found on the way. Three older entries moved to `docs/archive/`.

## 2026-11-05_105833_1793872713: Version 1.1.76: engine backlog items (fetch deadline, upload mode, web seed credentials, LSD hybrids)

- **Fixed:** `http_get_to` requires a streamed body to average at least 1 KiB/s after one timeout's grace (`MIN_STREAM_RATE`), so a slow-drip server cannot hold an update or helper download open forever.
- **Fixed (libtorrent `set_upload_mode`):** entering upload mode cancels every peer's outstanding requests (`cancel_all_requests`, also used by `snub`); leaving it clears the peers' reconnect delays. Dropping all at once, the block in flight included, is noted in PORTING.md. Wire behaviour change.
- **Fixed:** web seed URLs with credentials show `user:***@` in the peer list, log lines and the `UrlSeed` alert text; the engine, resume data and the `WebSeeds` alert keep the real URL (PORTING.md).
- **Fixed (libtorrent `lsd_announce`):** hybrid torrents announce both the v1 and the truncated v2 hash on LSD.
- The SChannel stale-key test uses a per-run key name. Skipped: resetting the reconnect delay on unskip (libtorrent 2.0.11 does not either), and `hash_request_cost` (already right since 1.1.33; the BACKLOG note was stale).
- Tests for each; clippy and all core tests pass.

## 2026-11-05_104753_1793872073: Version 1.1.75: GTK row shading, Start/Stop/Pause/Enter keys, refused Relocate shown

- **GTK:** the torrent list, detail tables and RSS items shade every other row, as Win32 does: odd rows get `pa-alt` by position (bind and position changes; `nth-child` would count only rows on screen), and a CSS rule is on the display while `ui.alternate_rows` is on. UI Settings shows the box on GTK now.
- **GTK:** Ctrl+S / Ctrl+T / Ctrl+Shift+P start, stop and pause; Enter in the torrent list opens Properties (a double click still opens a finished torrent's folder), as on Win32.
- **GTK:** a Relocate the engine refuses shows an error dialog besides the Logger line (new string `CannotRelocate`, all 35 tables, machine translations). Files tab Ctrl+A was already done by D50 (1.1.56).
- Clippy for the Linux target from Windows and the frontend tests pass; not run in a GTK session yet.
