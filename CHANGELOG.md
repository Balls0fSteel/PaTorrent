# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-11-03_123456_1793705696: Version 1.1.45: ports for profiles (D42 slice 2)

- **Listen port:** a new named profile, or a portable folder whose `patorrent.ini` is still empty, writes a random port (10000-60000, `random_port()` narrowed from 65000, the Web UI's button too) at its first start; the default profile keeps 6881. nox keeps no settings file outside the Web UI, so a named nox profile without `--port`/`--listen` takes a port from its name (FNV-1a, 10000-60000, the same at every start).
- **Port in use:** the engine does not fail on a taken port, it moves to the next one (as libtorrent's `max_retry_port_bind`), where nothing is forwarded. Both GUIs' network part now says "Port N in use" when TCP bound elsewhere than the configured port (or failed with "address in use"); a click opens Preferences at Connection (Win32 `preferences::show_at`, GTK `show(.., page)`). `status::ListenProblem`, `NetPortInUse`.
- **UPnP:** `SessionParams.label` (`PaTorrent (work)`) is the routers' mapping description for a named profile; the default profile keeps the user agent. `Protocol` is exported from core; every `SessionParams` literal gained `..Default::default()`.
- **Web UI:** the Win32 page proposes the first free port from 8080 up while the Web UI is off and the stored port is the default; a bind failure says "port N is in use, perhaps by another PaTorrent profile" (`WebUiPortInUse`), nox prints the same.
- **Checked:** unit tests (the listen tracker, free port, name port, labels); 1,536 workspace tests; clippy for four targets. Hidden desktop: two copies set to port 45123, the second (profile `work`, bound on 45124) shows "Port 45123 in use"; a fresh portable folder wrote `lt.listen_interfaces=0.0.0.0:19763,...` at its first start. Not clicked on the hidden desktop; GTK not run.

## 2026-11-03_121953_1793704793: Version 1.1.44: `--profile` in all three programs (D42 slice 1)

- **New:** `--profile NAME` (or `--profile=NAME`, anywhere before `--shell`, never a torrent) runs a profile of its own: `profiles/<name>` before the program's folder (`%APPDATA%\PaTorrent\profiles\work`, `~/.config/patorrent/profiles/work/gtk`, `.../profiles/work/nox`, the same inside a portable folder). Names are 1-32 of `a-z`, `0-9`, `-`, starting with a letter, case ignored; `default` is the default profile, whose folders do not change.
- **GUIs:** a profile without a folder is made after a question (`ProfileCreate`); a bad name is refused with a message (`ProfileBadName`); both machine-translated into the 34 tables. Titles, tray tooltips and About show `[work]`. Win32: its own mutex, window class and taskbar id (`PaTorrent.work`); GTK: its own application id (`<APP_ID>.profile_work`) and skips the name in a second start's arguments. Remote windows start in the same profile.
- **nox:** `--profile` changes the default state and config directories only (the flags and systemd's still win), created without asking; the `* state` line names the profile.
- **Checked:** new unit tests (names, argument scanning, folders, nox's and GTK's paths, nox's parser); 1,533 workspace tests; clippy for Windows, Linux (GTK), macOS (GTK) and FreeBSD. On the hidden desktop, a portable copy asked before making `profiles\work`, refused `Work!`, and ran `--profile work` (title `PaTorrent 1.1.43-dev [work]`, its own `patorrent.lock`) beside the default copy; `patorrent-nox run --profile work` used `profiles\work\nox`. GTK not run on a desktop.

## 2026-11-03_120646_1793704006: Version 1.1.43: one window per folder, `patorrent.lock` (D42 slice 0)

- **Fixed (1.x bug):** a second start of the installed copy could hand its torrent to a running portable copy, or the other way round: both looked for the window class `PaTorrentMainWnd`. Every folder but `%APPDATA%\PaTorrent` now has its own class (`PaTorrentMainWnd.<FNV-1a 64 of the folder>`); the installer's check still finds the installed copy.
- **New:** `patorrent.lock` in the settings folder (Win32), the library folder (GTK) and the state directory (nox), held for the process's life (share mode 0 on Windows, `flock` on Unix, both honoured over shares). A folder opened from another session or computer shows "Another PaTorrent is already using this folder" (`FolderInUse`, 34 machine translations) and exits; nox says so on stderr.
- One portable test for the GUI and nox (`frontend::profile::portable_root`, `patorrent.ini` must be a file).
- **Checked:** workspace tests (new: the class name, the lock, the marker); clippy for Windows, Linux (GTK too), macOS and FreeBSD; a second `patorrent-nox run` on a held state directory exits 1 with the message; a portable `patorrent.exe` with its lock held from another process shows the message on the hidden desktop.

## 2026-11-03_115032_1793703032: Profiles and portable mode approved: the plan (D42)

- **Owner's request:** the best approach for [PROFILES.md](docs/PROFILES.md) by use case, and a plan to implement. [PROFILES_PLAN.md](docs/PROFILES_PLAN.md) answers Q1-Q9 (the `profiles/<name>` rule, `[a-z][a-z0-9-]{0,31}` names, a File > Profiles menu and no environment variable, random ports for new profiles and the default's 6881 kept, associations owned by one profile, profiles in the Flatpak, relative paths converted on write in portable mode only, macOS portable mode last, slice 0 in 1.1.x) and lays out slices 0-8 with the files and functions each touches. D42 moved to the active decisions; D7 amended.
- **Found while mapping the code:** `resume.db` is the engine's, so relative save paths are converted in core (`SessionParams` gains the portable root and a label for the UPnP description); both GUI parsers would take `--profile NAME`'s value as a torrent; remote windows relaunch without the profile; nox has no `service status` (added for the installer's checks); the Flatpak's `--filesystem=home` lets a profile write its own desktop entry; the lock needs no new `windows-sys` features (share mode 0 / `flock`, as nox's pid file).
- Docs only, no version bump. The five oldest entries (1.1.33-1.1.35, the numbered scripts) moved to `docs/archive/CHANGELOG-2026-11-02--2026-11-02-v1.1.33-files-tab.md`.

## 2026-11-03_113345_1793702025: Version 1.1.42: the closing window skips the trackers and fits its content

- **Owner:** the 1.1.41 fix works on a real PC. The closing window (D47, amended) showed for the trackers' "stopped" announces, with a large empty area where the moves' list was hidden.
- **When it shows** (`frontend::closing::ShowTimer`, both GUIs): once one step has taken a second, each step starting the clock again, and never for the trackers' wait (at most `stop_tracker_timeout`, 5 s; a second start meanwhile already waits for this one to end). A window already shown stays; a Windows sign-out or desktop logout shows it at once as before.
- **Size:** Win32 lays the window out for what it shows: the step's text as tall as its lines, the list only while there are moves, the window's height to fit, centred where it was. GTK has no fixed height any more and shrinks back when the list goes (type-checked and linted only; not run on a desktop).
- **Checked:** the demo's slow exit captured on the hidden desktop: 541×321 with the list, 158 for a two-line step, 145 for one line; without moves nothing showed during saving and the trackers' 2 s. Workspace tests (1,525), clippy, GTK lint for Linux.

## 2026-11-03_111113_1793700673: Version 1.1.41: a stopped partial download counts only its selected files

- **Owner's note:** a torrent with one file of a hundred selected showed 5 % when stopped and 100 % when started. **Cause:** with a session store, stopping drops the metadata (D5), and without it `wanted()` and `is_finished()` fell back to the whole torrent, so Done, Size and Remaining counted every file and the torrent was not Finished (also for sidebar categories, seeding goals and auto-shutdown). A restored stopped torrent, which never loads its metadata, had the same problem. µTorrent and qBittorrent count only the selected files, stopped or not, as our running state already did.
- **Fix:** `Torrent::drop_metadata` keeps `wanted_pieces` (the pieces a downloaded file touches, boxed, only when some piece is not wanted), saved as `lt-wanted-pieces` beside the other `lt-` listing keys. Records from before 1.1.41 with a skipped file have their metadata loaded once at start to work it out and are saved again. Changing file priorities on a stopped torrent loads its metadata first. `Torrent` grows 8 bytes (688).
- **Checked:** new swarm test (running, stopped, restored stopped; it fails without the fix with 1,312,345 wanted bytes against 362,073) and unit test (no metadata, resume round trip); workspace tests, clippy, the fuzz workspace.

## 2026-11-03_103712_1793698632: 1.1.40 published

- **Owner's go-ahead:** the draft published as Latest (2026-11-03); the website branch merged into `main` and deployed: the "It shares your PC" section with the gamer mode and smart disk I/O cards, the 1.1.40 news entry. Checked live: `update.txt` `stable=1.1.40`, the home page's download button and section, `news.atom`.
- The release notes and metainfo moved from 1.1.35 (drafted first) to 1.1.40 as smart disk I/O landed before the release; 1.1.35-1.1.39 were never published.

## 2026-11-03_102201_1793697721: 1.1.40 release checks

- **CI** found three tests that only held on this PC, none a product bug, all fixed without a version bump: the drive override test assumed the temp folder is not on a hard disk (GitHub's Ubuntu runner's is); the Files tab path test expected `C:x` to be refused off Windows, where it is a plain name (since 1.1.34, first pushed now); the release job's live tests failed where nothing answers pings (GitHub's Windows runner) and outside the bloated-line setup, which now skip. CI, FreeBSD and Fuzz green on `b300136e`, tagged `v1.1.40`; the Release workflow built all 14 files.
- **Clean Windows VM, the draft's files** (checked against `SHA256SUMS`): `zip-test.ps1` all ok (both programs run, Web UI logins, portable mode leaves `%APPDATA%` alone, no Defender detections); `setup-test.ps1 -OldVersion 1.1.17` 0 failures (upgrade, refusal while running, one Apps entry, uninstall, all users). The VM was left clean.
- **Benchmarks** on the Xubuntu VM (now 4 GB): no regression against 1.0.1 run the same day; the empty session's extra 6 MB is nox's update check (1.1.17), recorded in BACKLOG. Details in BENCHMARKS.md.

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
