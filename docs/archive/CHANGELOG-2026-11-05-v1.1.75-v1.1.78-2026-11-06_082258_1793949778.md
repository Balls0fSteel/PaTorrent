# Changelog archive: 2026-11-05, v1.1.75-v1.1.78

Moved from the root CHANGELOG.md on 2026-11-06_082258_1793949778. Paths and links keep their repository-root context.

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
