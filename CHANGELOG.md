# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-11-03_090534_1793693134: Version 1.1.40: FreeBSD hard disks; the VM checks

- **Fixed:** FreeBSD never told a hard disk from an SSD: `kern.geom.confxml` puts `<rotationrate>` in the provider's `<config>`, and the parser only read the geom's own (the test fixture had it there). It now searches the whole geom; a fixture shaped like FreeBSD 15.1's output and the VM's real XML (da0, 15000 rpm: HDD) pass.
- **Xubuntu VM (1.1.39):** core disk tests 58/58; detection: `/` and loop/dm-over-loop disks HDD (VMware's disk says rotational), tmpfs and `/proc` unknown, a path not made yet takes its ancestor's drive; the check thread runs at the idle I/O class during a recheck and returns to none; a 48 MB recheck at 10 MB/s took about 5 s; GTK tests 28/28, the Disk I/O page and its settings kept across a restart, the Logger's `Disk I/O: drive /dev/sda (HDD)`. No network mount there (the Samba share needs the owner's password).
- **FreeBSD VM:** core disk tests 57/57; everything is ZFS (unknown, by design), which also shows the `statfs` offsets read right.
- **GTK hand check of 1.1.34-1.1.35** (Xubuntu, 1.1.37): the Files tab's whole menu, Prioritize by File Order, Priority, Don't Download, Relocate (moves; refuses to overwrite), Open through the portal and Open Containing Folder all work and survive a restart; "Append the torrent's label" saves and a real download from a local seed moved to `done/Movies`. UX notes in BACKLOG "GTK desktop checks and limitations".
- **Web UI** (nox mock, browser): the Disk I/O page's controls, the greyed MB/s number and a saved change; the Bandwidth page's new labels.

## 2026-11-03_085337_1793692417: Versions 1.1.36-1.1.39: smart disk I/O (D48)

- **Owner's choices:** build D48 before the release, all four parts; gentle checks and drive tuning on, the busy-disk throttle off; our own timings only; uploads and downloads as separate boxes; overrides in the Advanced table only; a new Disk I/O page. Details and what was simplified: [DISKIO.md](docs/DISKIO.md#implementation-status).
- **1.1.36:** the settings (`check_gently`, `check_rate_limit` / `check_rate_mb`, `tune_for_drive`, `drive_overrides`, `disk_slow_downloads` / `_uploads`), `active_checking` default 4, remote protocol 13.
- **1.1.37, engine** (`disk/smart.rs`, `engine/disk_limit.rs`): `lt-hash` routes each check to one of eight `lt-check-N` threads by drive (one check per drive, `active_checking` drives at once); gentle checks run at background I/O priority on their own handles with read-ahead/forget hints, wait after a read twice as slow as usual, and keep under the MB/s cap; hard disks and removable drives take two jobs at once, a share's server four; with a throttle box on, writes and peer reads are timed per drive and a busy drive halves the download or upload (one global cap, merged with keep ping low's, the lower wins; "(auto ...)" in the status bar, Logger lines). The drive is cached on the storage and nothing is timed while both boxes are off.
- **1.1.38:** Preferences > Advanced > Disk I/O in Win32, GTK and the Web UI, 11 strings in all 35 tables (machine translations; Win32 page seen in 9 languages, nothing clipped). Fixed on the way: the Web UI's Bandwidth page showed `rate_limit_ip_overhead`, `rate_limit_utp` and `auto_rate_limit` by their raw names (since 1.1.29).
- **1.1.39, detection** (`disk/drive.rs`, `disk/gentle.rs`): Windows by volume extents and seek penalty (no elevation; UNC and mapped drives are Network without touching the server), Linux by `statfs` and sysfs (`rotational`, partitions, `dm`/`md` slaves, USB as removable), macOS by IOKit's medium type, FreeBSD by `kern.geom.confxml`'s rotation rate. This PC: C: and L: SSD, the mapped V: network.
- **Checked:** 1,517 workspace tests, clippy for Windows, Linux, FreeBSD and macOS (core), GTK from Windows with the stand-in pkg-config, the fuzz workspace. The VM runs are in the 1.1.40 entry.

## 2026-11-03_082759_1793690879: 1.1.35 release prepared

- **Release (owner: draft the notes, metainfo and news):** `docs/releases/1.1.35.md` covers everything since 1.1.17 (1.1.18-1.1.35): keep ping low, the Windows self-update and the once-per-version box, the closing window and safer sign-out in both GUIs, the Files tab's 2.2.1 menu, label folders, the libtorrent 2.0.12-2.1.2 fixes, the engine options and the release review's security fixes. Peer ID `-PA1012-`, remote protocol 12.
- Flatpak metainfo has a 1.1.35 `<release>` dated 2026-11-03; change the date if it is published later. The news entry is on the branch `news-1.1.35`, to push when the release is published (RELEASING.md step 2).
- Docs only, no version bump. Still to do before tagging: push for CI, rerun `tools/bench`, and a hand look at the 1.1.34-1.1.35 Files tab changes, which came after the release review.

## 2026-11-02_231601_1793657761: Numbered Windows scripts and 3_build_and_setup.bat

- **Owner's request:** a script to build and run the setup, to upgrade the PaTorrent installed on this PC, and numbered script names. `build.bat`, `run.bat`, `run-demo.bat` and `fuzz.bat` are now `1_build.bat`, `2_run.bat`, `4_run_demo.bat` and `5_fuzz.bat`.
- **`3_build_and_setup.bat`:** `1_build.bat`, then `makensis` (as the release does, the version read from `dist\patorrent-nox.exe --version`) writes `dist\patorrent-<build>-windows-x64-setup.exe` and runs it; the setup upgrades the installation it finds. Arguments go to the setup (`/S` silent). Checked on the hidden desktop up to the setup's language page.
- **Found:** cmd does not run a name starting with a digit without a path (`call 1_build.bat` fails; `.\1_build.bat` works), so the scripts call each other by full path and README says to type `.\`. And `build.bat` never set its `-dev.<commit>.dirty` suffix: inside `for /f`'s command cmd turns `=` into a space (`--short 7`, `--untracked-files no`), so builds fell back to `-dev.<commit>` from git without `.dirty`; now `^=`.
- Scripts and docs only, no version bump. The five oldest entries (D48-D49 proposals) moved to `docs/archive/CHANGELOG-2026-11-02--2026-11-02-d48-d49-proposals.md`.

## 2026-11-02_230210_1793656930: Version 1.1.35: the Files tab's menu as µTorrent 2.2.1's

- **Win32 and GTK:** Open, Open Containing Folder | Prioritize by File Order, Priority (High, Normal, Low) | Don't Download | Relocate..., from the owner's 2.2.1 screenshot. Left out: Scan for viruses (no scanner to run). Open, Open Containing Folder and Relocate are hidden in a remote window; Relocate takes one file.
- **Prioritize by File Order** (`detail::by_file_order`): the selected files, in the torrent's order, get priorities 7 down to 1 (shown as High, Normal, Low), so they finish in order; more than seven share levels in runs.
- **Relocate** asks for a path (no replace prompt on Win32) and calls `rename_file` with it: a file on disk moves (never over an existing file); a missing one is looked for there, and a Force Re-check counts it.
- **Checked:** the Win32 menu and its submenu captured on the hidden desktop from the demo (popup menus are tool windows, which `run_hidden`'s own screenshots skip). GTK type-checked and linted only. Strings `MenuPrioritizeByOrder`, `MenuRelocate`, `PickRelocate` in all 35 languages (machine translations).

## 2026-11-02_225455_1793656495: Version 1.1.34: the Files tab's Open, and label folders

- **Files tab (Win32, GTK):** a double click or Enter on a file opens it in its default program, and the menu starts with µTorrent's Open and Open Containing Folder (the folder selects the file, or opens the nearest existing folder inside the save path while it is not on disk yet). A file not on disk does nothing. Paths come from `detail::file_path`, which takes only plain names under an absolute save path (as SEC-18's Open Folder). Hidden in a remote window. Not run by hand: an agent cannot open programs on this desktop.
- **"Append the torrent's label"** (µTorrent's `dir_add_label`, default off; the owner's own settings had it on): beside "Move completed downloads to:" in Win32, GTK and the Web UI (`dir.move_add_label`). A labelled torrent that finishes moves to `<completed>\<label>`; none or an empty label, to `<completed>`. `label_dir` keeps a label inside the folder: `/` and `\` nest, Windows-forbidden characters become `_`, `.`/`..`/empty names drop, device names get a `_`, names stop at 255 bytes. `MoveCompleted::on_alert` takes a label lookup and returns the destination. A real labelled move passes in `tests/move_storage.rs`.
- **Strings:** `MenuOpenFile`, `PrefMoveAddLabel` in all 35 languages (machine translations).

## 2026-11-02_223318_1793655198: Why qBittorrent "refused" our uTP connections

- **Cause:** right after a torrent's check, qBittorrent keeps it paused in libtorrent for a few seconds while its API says `stalledUP`; with no other active torrent libtorrent drops incoming connections ("no active torrents", no alert), which for uTP is a FIN acking our SYN, and we then wait 15 s before TCP, as libtorrent does. Found from libtorrent's source, a capture, a 5 s pause and an unrelated active "keeper" torrent (each makes every run connect over uTP at once), and plain libtorrent 2.1.1 accepting every time. No engine change: `tools/qbt-interop` waits 5 s after "checked" (then 0.5-1 s "down" runs over uTP on 2.0.15). BACKLOG records the optional quicker give-up.

## 2026-11-02_221313_1793653993: Version 1.1.33: the release checks for 1.1.18-1.1.32

- **Security review** of everything since v1.1.17 ([SECURITY-REVIEW.md](docs/SECURITY-REVIEW.md#release-1132-since-v1117)), 15 Low findings fixed, each with a test: the self-update runs the setup only as checked (locked and hashed again before it starts), writes no `.part` through a planted link, refuses https-to-http redirects, and its streamed fetch stops at the length and the limit; a SEC-26 bypass through a URL-parser mismatch in search downloads (older than 1.1.17); watched folders refuse oversized files before reading; duplicate file names resolve in linear work (8,000 took 29.8 s); resume `pieces` read back as saved; ICMP replies only from the host pinged, IP headers and the Linux error queue read within bounds; `hash_request_cost` counted one proof sibling too few (a piece-layer request with proofs cost 0 against the hash budget; a full piece-layer fetch now measures `P × (log2(P) - 8)`). Hardening: RSS validators, qBittorrent share limits, version length. New fuzz target `icmp` (10.8 M runs), `web_seed` takes a referrer. Open items in BACKLOG "Security and TLS".
- **Interop** against qBittorrent 5.3.0 on libtorrent 2.0.15 and 2.1.2 (new `tools/qbt-interop`): v2-only and hybrid with odd file sizes 48/48, forced RC4 8/8, slow-torrent queueing as specified. No engine change. Found: qBittorrent often refused our outgoing uTP with a FIN (explained in the entry above).
- **Windows, on the clean VM:** `setup-test.ps1` upgrading the published 1.1.17 setup: 24/24. With new `tools/windows-clean/desktop` scripts, driven in the locked console session by window messages: the self-update for one user end to end (box, progress, Cancel, install, restart not elevated) and the closing window's Quit Now during real 12 GB / 4 GB moves (copies deleted, files whole, a restart seeding). Not scriptable: the all-users update's UAC prompt.
- **Update progress text:** sizes in the usual units ("24.1 % (768 kB / 3.11 MB)"), not whole MB ("0 / 3 MB").
