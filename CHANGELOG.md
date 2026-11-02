# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-11-02_111726_1793614646: Keep ping low proposal (D49)

- **Owner's request:** make the game-ping question a proposal. Written as [LOWPING.md](docs/LOWPING.md), proposed D49, questions Q1-Q7; no code.
- **Design:** one setting, off by default. ICMP probes to the first router past the home line (found by TTL; no admin rights needed on Windows, Linux and macOS, FreeBSD has none) plus the uTP peers' one-way delays (`send_delay` / `recv_delay`, already measured); the upload is capped at 80% of its rate when the queue stays over 15 ms and grows back 5% a second, with floors. It reuses D48 part 4's controller. No game list: the queue covers every game and call.
- **Prior art checked:** Tixati's auto throttle (caps sinking far too low, fixed by longer averages), Azureus AutoSpeed, cake-autorate on routers.

## 2026-11-02_111330_1793614410: Smart disk I/O: the owner's first answers (D48)

- **Q3:** disk I/O only; CPU load is fine. **Q8:** "smart recheck" means gentle, not fewer. DISKIO.md part 1 now uses every platform tool: background I/O and memory priority (Windows), `ioprio_set` idle per thread (what `ionice -c 3` does; Linux), `IOPOL_THROTTLE` (macOS); keeping other programs' file cache (`posix_fadvise`, `F_NOCACHE`); giving way when the check's own reads slow down, which works on NVMe and a NAS where the priorities do nothing; and the cap.
- **Ping for games:** answered in BACKLOG (networking): a delay-measuring upload limit works without a game list; uTP's LEDBAT already backs off but at 100 ms. Also recorded: our Bandwidth page lacks 2.2.1's "Global Rate Limit Options" group.

## 2026-11-02_110639_1793613999: Smart disk I/O proposal (D48), search engines and themes reviewed

- **Owner's idea:** smart recheck, throttling reads and writes on high disk I/O, SSD / HDD / NAS detection. Written up as [DISKIO.md](docs/DISKIO.md), proposed D48, questions Q1-Q8; no code.
- **Findings that shaped it:** all checks run on the one `lt-hash` thread, one after another for every drive, at normal priority and uncapped; `active_checking` is in `Settings` but unused; the `aio_threads` take jobs by `piece % threads`, so all ten can seek on one HDD; the write back-pressure (`disk_backlogged`) slows downloads to the disk's pace but leaves no room for other programs; nothing detects drive types.
- **Search engines:** 2.2.1's "Search Engines [name|URL]" list (Advanced > UI Extras) is not built; worth it with an empty default list (BACKLOG "V1.1"). The BACKLOG's search entry still said the Torznab UI was not built; corrected.
- **Themes:** options recorded in BACKLOG "Themes": icon skins first, a colour file, Win32 dark mode as a separate decision (undocumented `uxtheme` calls). 2.2.1's skin file names could not be read from its UPX-packed exe.
- Docs only, no version bump.

## 2026-11-02_092152_1793607712: The closing window in the GTK GUI (1.1.28, D47)

- **Owner's request:** build the GTK part.
- **frontend:** new `closing` module with the window's texts and rules (step, rows, the bar's fraction, what Quit Now gives up, when it is offered, when the window can go); the Win32 window now uses it, and its tests moved there.
- **GTK:** new `closing` module: `shutdown()` hands `Session::begin_shutdown` to it; it holds the application, inhibits logout and suspend (reason `EndSessionSaving`), shows "PaTorrent is closing" after a second (at once after `query-end`) with the step, a scrollable grid of moves (names cut at the end, folders in the middle, tooltips), one bar (pulsing for steps it cannot measure), the time and Quit Now (also the close button), which asks in an `AlertDialog`. `register-session` and `query-end` quit as File > Exit does; so do SIGTERM and SIGINT, which killed the GUI before with the resume data unsaved.
- **Run on Linux** (Xubuntu VM, GTK 4.22, debug build; `tools/closing-test`, new): real moves to a loop-mounted filesystem with reads throttled to 25 MB/s. SIGTERM brought the window up with one move running and one waiting; left alone, the moves finished and the app ended cleanly; Quit Now listed three moves, deleted the partial copy, kept the sources and exited 2.2 s after the click; a restart found every torrent seeding. Xfce's portal refuses logout inhibits, so an Xfce logout does not wait (BACKLOG). A first try with a 200 MB memory cap got the debug build OOM-killed in the scope: the sources were intact and only a partial copy was left at the destination, as a crash should leave it.
- **Checked:** all workspace tests without GTK here, GTK's 28 tests on the VM, clippy (core for Linux and GTK with all targets) and fmt.

## 2026-11-02_090224_1793606544: The closing window lists many moves (1.1.27, D47 amended)

- **Owner's question:** what if 15-20 torrents are stuck moving at exit? The moves were one static text, which would overflow, and the Quit Now question named every move.
- **Win32:** the moves are a scrollable owner-data list view (Name, Done, Moving to; label tips show truncated paths), hidden when there are none; one bar shows all the moves together. The question names two moves and then "{count} more torrents stop moving in the same way." The window is 300 x 160 dialog units.
- **core:** queued moves are listed too ("Waiting"): `DiskIo::async_move_storage` registers the move (destination, the torrent's size) and the disk thread marks it started, so the engine's list needs nothing per torrent. A first try kept the destination in `Torrent` and failed the D21 size test (`a_stopped_torrent_stays_compact`, 24 bytes per torrent); the registry is empty while nothing moves. `ExitMove::started` is new. The demo's slow exit now moves 20 torrents three at a time.
- **Strings:** `ExitMoveLine` replaced by `ExitColMoveTo`, `ExitMoveProgress` and `ExitMoveWaiting`; new `ExitAbortMoreMoves` (count-neutral wording in languages without a single plural, by the translation subagent); all 35 tables.
- **Checked:** the new registry test and the window's text tests, all workspace tests without GTK, clippy (also core for Linux and GTK) and fmt pass; screenshots of the 20-move list and the shortened question in English and Hungarian.

## 2026-11-02_085028_1793605828: The closing window (1.1.26, D47)

- **Owner's request:** show what PaTorrent is doing while it closes instead of looking stuck, and have Quit Now say what it abandons; a flush of written data may need its own treatment.
- **Findings that shaped it:** the resume data records only blocks already written (`unfinished_pieces` takes `Finished` blocks), our write queue is capped by `max_queued_disk_bytes` (1 MiB by default, peers stop reading above it), and data handed to Windows is written out by Windows even after the process ends. So the queue can be abandoned (those blocks are downloaded again); the step that must finish is saving the resume data, which gets its own text and no Quit Now. A cross-volume move copies every file before deleting any source, so a move cancelled inside the engine leaves the old folder whole.
- **core:** `Session::begin_shutdown` returns an `Exit` (`status`, `hurry`, `is_finished`, `wait`, `abandon`); the engine publishes `ExitPhase` (Stopping, Moving, Saving, Trackers, Closing), the moves in flight (`DiskIo::moves`) and the unwritten bytes. `hurry` cancels the moves (`DiskIo::cancel_moves`: the copy stops, copies are deleted, renames rolled back, the torrent keeps its folder, `StorageMovedFailed` with `Interrupted`) and skips the trackers' wait. Cross-volume copies use `CopyFileExW` with a progress routine on Windows and a 4 MiB chunked copy elsewhere. A failed move now also deletes the copies it made. `MockConfig::slow_exit` simulates a slow exit for `--demo` (`PATORRENT_DEMO_SLOW_EXIT`).
- **Win32:** new `closing` module: a modal "PaTorrent is closing" window after 1 s (at once at a sign-out, carrying the block reason, so a tray-only PaTorrent is waited for too), refreshed every 200 ms: the step's sentence, one line per move ("name: 2.01 GB of 8.00 GB to folder") with a bar, "Closing for m:ss", and Quit Now (also the close box), which asks with the list of what it gives up. 13 new strings in all 35 tables (the 34 translations by a subagent, checked for placeholders, escapes and access keys).
- **Checked:** new tests for the cancelled move, the cancellable copy, the background shutdown (`tests/exit.rs`) and the window's texts; workspace tests without GTK, clippy `-D warnings` (also core for the Linux target and GTK with the stand-in pkg-config) and fmt pass. Seen on screen with the demo's slow exit: the window after 1.1 s, the move line and bar, the Quit Now question, Saving with the button disabled, and the process gone 2 s after Yes. Not run: a real move to a NAS, a real sign-out (BACKLOG).

## 2026-11-02_083027_1793604627: Safer exit, sign-out and relaunch (1.1.25)

- **Picked from PLAN:** the code-ready startup/shutdown fixes from the 2026-10-30 findings (BACKLOG "Startup and shutdown"). The bound on the waits and a shutdown progress window stay open.
- **Windows sign-out and shutdown:** `WM_QUERYENDSESSION` registers `ShutdownBlockReasonCreate` with the new string `EndSessionSaving` ("Saving the torrent list and progress...", all 35 tables, 34 translated here); `WM_ENDSESSION` saves the settings and stops the engine (resume data first) before returning, with the window left visible so that Windows keeps waiting, then destroys it. `on_destroy` and this share `close_session`. Windows waits only for a visible window; from the tray PaTorrent still has the system's few seconds.
- **Relaunch during an exit:** the first instance now owns its mutex until the process ends and marks its window with the `PaTorrent.Exiting` property while it closes. A second start that finds the mark, or a window that stopped answering and is closing, waits for the mutex (no bound: an exit may wait for a move) and runs as the first instance, keeping its torrents; before, it gave up after 5 s and exited silently. A starting first instance is now waited for on the mutex instead of a sleep.
- **core:** a check stops at the next file once its torrent is stopped (shutdown fences every torrent): the resume stat pass, the "any file on disk" scan, empty-file creation and the part-file export (which leaves the rest in the part file for the next check). Before, only the hashing loop looked. No wire change, peer ID unchanged.
- **Auto-load off the GUI thread:** `torrent_folders::Watcher` runs the `Poller` scan and reads the handed-out files on a thread, one scan at a time, collected on a later tick; the Win32 and GTK GUIs use it (nox keeps the `Poller`, its loop is not a GUI).
- **Checked:** new tests `a_stopped_check_touches_no_more_files` (core) and `the_watcher_scans_and_reads_off_the_calling_thread` (frontend); workspace tests without GTK, stable clippy `-D warnings` and fmt pass; GTK linted from Windows with the stand-in pkg-config. Not run: a real sign-out, a relaunch during a slow exit, a folder on an unplugged share (BACKLOG hand checks).

## 2026-11-01_070555_1793513155: Microsoft Store release planned (proposed D46)

- **Owner's question:** what a Store release needs, whether listing is free, and whether the Store build can be paid while GitHub stays free.
- **Findings (checked on the web 2026-11-01):** registration is free for individuals and, since May 2026, companies; MSIX is signed and hosted by the Store for free and must not self-update; an EXE listing needs our own Authenticode signing and, as far as documented, our own payments; the 2022 open-source sales ban was withdrawn. The owner is the sole copyright holder (PolyForm Noncommercial binds others), so a paid build is allowed.
- **Docs:** [STORE.md](docs/STORE.md) (an MSIX `PATORRENT_STORE` flavour, the package, the listing, Q1-Q6), D46 proposed in DECISIONS.md, PLAN item F, a BACKLOG note. No code; no version bump (docs only).
