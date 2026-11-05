# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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

## 2026-11-05_102610_1793870770: Version 1.1.74: the bug hunt's engine low findings fixed (18-23, 36, 38)

- **Fixed (18):** a PIECE that is not a valid block of the torrent (range, alignment, length) disconnects the peer, as libtorrent's `verify_piece` does, instead of holding the request until the snub. Wire behaviour change.
- **Fixed, uTP (19, 20):** a close while the UDP socket is stalled sends its FIN once writable; a stalled socket's deferred ACK no longer takes the manager's single slot (the debug assertion stays).
- **Fixed (21):** a relative redirect with `://` in its query is resolved against the request.
- **Fixed (22):** an unmatched NAT-PMP reply no longer cancels the resend timer and leaves the mapper stuck.
- **Changed (23):** a web seed's `Retry-After` is honoured even when shorter than `urlseed_wait_retry` (at least 1 s, added at merge so `Retry-After: 0` cannot loop). libtorrent 2.0.11 raises it; PORTING.md "Web seed errors" updated.
- **Fixed (36, 38):** an out-of-range DHT node port in a .torrent is skipped, not truncated; `MerkleTree::add_hashes` on a file of at most half a piece and `load_sparse_tree` on a 0-block tree no longer panic (the third cited panic, `get_piece_layer` for one block, did not reproduce).
- Each with a test (tests for 18, 19, 20, 23 failed before the fix); clippy and all core tests pass. All 21 low findings are now closed: 20 fixed, 35 ruled out (1.1.73).
- **Validated (2026-11-05_102905_1793870945):** on the Xubuntu VM, `patorrent-gtk` passes clippy and the core (912), frontend and nox tests pass. That run caught a flaky `a_taken_port_is_not_reused`: a parallel test can take the freed ephemeral port first. It now retries with fresh ports (test-only, no version bump). BACKLOG, BUGHUNT and PLAN updated; PLAN proposes peer ID `-PA1013-` for 1.2.0.

## 2026-11-05_102244_1793870564: Version 1.1.73: the bug hunt's disk and platform low findings (24, 25, 34; 35 ruled out)

- **Fixed (24):** on Windows, renames and cross-volume moves past MAX_PATH no longer fail: `MoveFileExW` and `CopyFileExW` get the `\\?\` / `\\?\UNC\` form as `std::fs` does (`file_pool::wide_path`). Real long-path rename and copy tests fail without it.
- **Fixed (25):** a part-file export into a file not yet on disk writes `<name>.pa-export`, syncs and renames it once whole; the part file frees the pieces only then, so an interrupted export runs again on the next start instead of leaving a short file.
- **Fixed (34):** the Windows pinger no longer leaks the ICMPv6 handle when ICMPv4 is unavailable.
- **Ruled out (35):** SChannel creates no server context while a ClientHello is incomplete (probed: `SEC_E_INCOMPLETE_MESSAGE`, handle 0/0). Two tests pin it, one a split-hello handshake on every TLS backend.
- clippy and all core tests pass (905 lib tests).

## 2026-11-05_101942_1793870382: Version 1.1.72: the bug hunt's nox, frontend and GTK low findings fixed (30-33, 37)

- **Fixed, GTK (30):** a Cancel that arrives while Create New Torrent's worker finishes discards the result and removes the written .torrent instead of seeding it. Read only on Windows; compiled on the Xubuntu VM with the rest (below).
- **Fixed, nox (31):** a named-profile Windows service fixes its profile from its options before resolving its directories (`service::prepare_with`), so it no longer runs as the default profile.
- **Fixed, frontend (32):** `Prefs::set_sort(None)` goes through the change journal, so the D40 mirror no longer keeps a stale `list.sort`.
- **Fixed, nox (33):** `paths::migrate` copies across file systems to a temporary name and renames it, so a failed copy leaves no partial `webui.json`.
- **Fixed, Web UI (37):** the ban list forgets a failure count whose last failure is older than the ban duration (at least 60 s). qBittorrent has no decay; the rule is ours.
- Tests for 31, 32, 33 and 37; clippy and the nox and frontend tests pass.

## 2026-11-05_101739_1793870259: Version 1.1.71: the bug hunt's Win32 low findings fixed (26-29)

- **Fixed (26):** a saved window rectangle whose title bar is on no monitor (one was unplugged) is moved, and shrunk where larger, into the nearest monitor's work area (`fit_rect`).
- **Fixed (27):** Search Sources Edit and Remove find their source again by its key after the modal box, so a Web UI change meanwhile no longer edits or deletes the wrong row; a source that has gone is left alone.
- **Fixed (28):** the scheduler grid ends a drag on `WM_CAPTURECHANGED` and on a mouse move without the left button.
- **Fixed (29):** Preferences trims the download folder like the other folder fields.
- Each with a unit test; clippy and `cargo test -p patorrent` pass. `search::tests::the_helper_buttons_follow_the_install` failed once in the agent's full run and passed on rerun (timing; not touched).
