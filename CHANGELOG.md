# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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

## 2026-11-05_091957_1793866797: Version 1.1.70: the bug hunt's twelve medium findings fixed

- **Fixed, Win32 and GTK (6):** opening Torrent Properties while a torrent finished, was added, failed or was removed lost that event: the alerts the dialog popped while it waited for the tracker lists were cleared by the next alert pass (`pop_all` empties the Vec it fills), so no balloon or log line, a removed torrent's row stayed all session, a pending Standby never fired. Both `on_alerts` take the shared `properties::take_alerts`, which pops behind the saved alerts.
- **Fixed, engine (7):** peers that connected while a magnet's metadata was fetched, or while the files were checked, never learnt our pieces and were never served until they reconnected. The piece set and allowed-fast set go out once the files are checked, to every peer still without them (libtorrent `files_checked`); a peer that handshakes before the check gets no HAVE_NONE first, as in libtorrent.
- **Fixed, engine (8):** a peer that had nothing for us for over 20 s was snubbed the moment we asked it for new blocks (CANCELs, one block at a time). The request timeout runs from the request that refills an empty queue (libtorrent `m_requested`).
- **Fixed, engine (9):** choking a peer dropped the requests we had accepted for its allowed-fast pieces without a REJECT, so it waited its timeout and snubbed us. They stay and are served through the choke.
- **Fixed, UPnP (10, 11):** after a listen-port or network change deleted a mapping, the lease refresh (lease × 3/4, 45 min at the default) re-armed itself in the past and the engine loop spun at 100 % CPU with a log line per iteration; `on_expire` clears the expiry first. Closing the mapper with an AddPortMapping in flight never sent the delete, so the mapping stayed on the router and shutdown waited out the stop timeout; the Map reply hands the slot on while closing, and `pending()` ignores a disabled device.
- **Fixed, daemon (12):** `--daemon` and the Windows service make `--webui-cert`, `--webui-key`, `--webui-save-root` and `--import-dir` absolute before the chdir, so a relative certificate no longer fails "not found" and a relative save root no longer makes the Web UI refuse every path.
- **Fixed, Win32 search (13):** when a source answered after a result was picked, the selection moved to whatever row the re-sort put at its old place, and Add fetched that one. The selected keys are read before the merge.
- **Fixed, GTK (14):** Exit from the tray, SIGTERM or Ctrl+C while the main window was already closing ended the program at once, abandoning the resume save; `quit_app` now waits for the closing window. Read and clippy-checked only (no GTK build here).
- **Fixed, fetch (15):** a `.torrent`, feed or search download failed with "invalid HTTP header" when a header carried a Latin-1 byte (a `Content-Disposition` file name), a `100 Continue` ended it as truncated and a `103 Early Hints` as "HTTP 103": the response is built from the lenient head that framed it, every 1xx head is skipped and non-UTF-8 header bytes are read as Latin-1.
- **Fixed, folder watcher (16):** a failed add (a duplicate) cleared every pending entry with its name, so a second auto-loaded file with the same name stopped the first from being marked `.loaded`, and a failed magnet (no name) made the watcher forget every other magnet's `.torrent` to store. `AddTorrentFailed` carries the info-hash where the source parsed (remote protocol 15; a `/pa/remote` add refused by the path policy carries it too) and only that pending is dropped.
- **Fixed, Win32 (17):** opening a torrent while the running PaTorrent was busy for over 5 s (a .torrent read from a slow share, a long alert batch) counted the timed-out hand-over as taken and the second start exited with the torrent. It keeps sending until the window takes it, or takes over if the window goes.
- **Checked:** fmt and clippy clean (workspace, all targets, GTK through the stand-in pkg-config); the core, frontend, nox and Win32 test suites; each new test failed before its fix. Built by four agents in worktrees, merged here; the report ([docs/BUGHUNT-2026-11-05.md](docs/BUGHUNT-2026-11-05.md)) records each fix and test; BACKLOG lists the 21 open low findings.

## 2026-11-05_085145_1793865105: Version 1.1.69: the bug hunt's five high findings fixed (security)

- **Fixed, Web UI (security):** the fixed bypass sessions `apikey`, `local` and `subnet` live in the login map, so a request from anywhere with `Cookie: SID=local` was logged in while a loopback, subnet or API-key client had been active within the session timeout. A cookie naming one of them is ignored now (`webui/api.rs`, the names as constants). Test: a proxied request with each name is 403 while the local bypass is in use.
- **Fixed, engine (remote crash):** `is_idna` sliced a web seed's host label by byte, so a `url-list` entry, a magnet's `ws=` or resume data with a label such as `aéb` panicked the engine thread and ended the session. It looks at the first four bytes only when they are whole characters. Test: unencoded labels, and `xn--` after one.
- **Fixed, metainfo:** a file name was borrowed from the torrent whenever the sanitized path merely ended with its bytes, so a name of the form `<tail of the parent>/<240 bytes>` was stored with its `/` (libtorrent's behaviour), or, starting inside a character, as an empty name whose path was the torrent's root directory. It is borrowed only as the path's whole last element. Test: both shapes, reproduced before the fix.
- **Fixed, disk:** a file-priority change, move, rename, stop or remove (a barrier or fence) went to every checking thread and waited behind an unrelated torrent's running check, hours under `check_rate_mb`; meanwhile the I/O threads parked that torrent's writes, still counted in the write queue, and the back-pressure zeroed every peer's quota. A running check now takes its queue between pieces and runs another storage's fences and barriers at once; everything else waits in order for the check to end. Test: a priority change for S is answered during T's check (failed before).
- **Fixed, remote protocol:** `/pa/remote` forwarded `AddTorrent` and `MoveStorage` to the engine without the path policy, so `--webui-save-root` did not bind a GUI's remote session. `remote::dispatch` takes the server's path check; a refused command is answered with the `AddTorrentFailed` / `StorageMovedFailed` alert and never reaches the engine; an empty save path takes the default folder, as the Web UI's add does. Test: an add and a move outside the root are refused, the torrent stays where it was.
- **Checked:** fmt and clippy clean (workspace, all targets); core 894 lib tests and its integration tests; frontend 283; nox 185 across its suites (the HTTPS remote test caught the empty save path); the Win32 crate's 101. The report ([docs/BUGHUNT-2026-11-05.md](docs/BUGHUNT-2026-11-05.md)) records the status; the 33 open findings are in BACKLOG "Bug hunt findings".

## 2026-11-05_082716_1793863636: Bug hunt over all five crates: 38 findings recorded, nothing fixed

- **Review:** ten parallel read-only reviewers, one per area (bencode and metainfo, engine and peers, uTP and net, disk, DHT/trackers/UPnP/web seeds, the frontend, the nox Web UI, the nox daemon and service, Win32, GTK), each finding re-read here at its lines; two reproduced by running the code. The report with file:line, scenarios and fix directions: [docs/BUGHUNT-2026-11-05.md](docs/BUGHUNT-2026-11-05.md).
- **High (five):** the Web UI accepts the fixed bypass names `apikey`, `local` and `subnet` as `SID` cookie values while such a client is active (`webui/api.rs:299-331`); a web seed host with a non-ASCII label panics the engine thread in `is_idna` (one `url-list` entry stops the session); a borrowed file name can span a `/` and, in the port, become empty, so a file's path collapses to the torrent root (`torrent_info.rs:1050`, reproduced); a file-priority change, move or rename waits behind every checking thread's current check while its parked writes trip the global disk back-pressure (`disk/mod.rs:762`); the `/pa/remote` protocol forwards `AddTorrent` and `MoveStorage` without the path policy, so `--webui-save-root` does not bind it.
- **Medium (twelve):** alerts popped by `fetch_trackers` are discarded by the next `pop_alerts` in both GUIs; peers kept across a magnet's check never get our bitfield; a refilled request queue after 20 s idle is snubbed at once; a choke drops accepted allowed-fast requests without a REJECT; a UPnP unmap leaves `expires` set and `on_expire` then spins the engine loop at 100 % CPU; a UPnP close with a Map in flight keeps `pending()` true; `--daemon` and the service never absolutize `--webui-cert`, `--webui-key`, `--webui-save-root` and `--import-dir`; the Win32 search re-applies the selection by stale indices after a merge; a GTK quit during the closing window hard-exits; `fetch.rs` frames with the lenient parser and builds with the strict one (a Latin-1 header fails a .torrent download); the folder watcher drops every pending entry sharing a failed add's name; a 5 s hand-over timeout to a busy first Win32 instance counts as taken.
- **No version bump:** documents only. The 1.1.63, real-Jackett checklist and Windows search guide entries moved to `docs/archive/CHANGELOG-2026-11-04--2026-11-04-v1.1.63-search-guide.md`.
