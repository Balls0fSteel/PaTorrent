# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-11-04_095851_1793782731: Version 1.1.56: the Files tab's quick filter (D50)

- **D50 (built by an Opus 5.5 agent, reviewed and merged):** the Files tab has a quick filter in all three UIs: a "Filter files" box at its top right lists only the files whose path (folders included) has every word typed, ignoring case, with "12 of 3,400 files" beside it. Escape clears it, Ctrl+F from the Files tab goes to it, Ctrl+A selects the shown files (Win32 and GTK used to select torrents there). The text stays when another torrent is selected and is never stored. Shared `frontend::detail::FileFilter`; `Detail.file_rows` holds the filtered rows, so the menus, Open, Relocate and the double click follow it. Prioritize by File Order orders all of the torrent's files while a filter is on. Two strings in every language table. Assumed answers: the text is kept across torrents and the box is always visible.
- **Checked:** fmt, clippy and the frontend, nox and Win32 tests here; GTK clippy and an Xvfb run with snapshots on the Xubuntu VM; a hidden `--demo 20` Win32 run with screenshots. Not run: the Web UI in a browser, Prioritize by File Order with a filter on, a remote window, 150-200 % DPI.

## 2026-11-04_091839_1793780319: Six proposals drafted for discussion (D50-D55)

- [docs/PROPOSALS.md](docs/PROPOSALS.md), from the owner's points: a Files tab quick filter (a shared row-to-file map over both virtual lists); a search helper that downloads the upstream Jackett release at the user's click and runs it on loopback only for a search (Jackett is GPL-2.0 and Prowlarr GPL-3.0, read from GitHub; bundling rejected, a Rust Cardigann interpreter kept as the later option); a native AppKit GUI as a fourth crate, after gate 8; a first-start Setup Guide reusing 2.2.1's menu entry (router age, smart disk I/O, folders); network binding as a rule (adapter, address range or "any VPN adapter") with pause-all when the network is missing, since WireGuard does create an adapter but not a stable one; help text as one keyed table with hover, a Preferences pane and F1, no per-control marker. PLAN step I.
- Docs only, no version bump. D48's smart disk I/O questions were answered 2026-11-03 and leave the plan's open list.

## 2026-11-04_083614_1793777774: Version 1.1.55: the GTK GUI's windows as files (`PATORRENT_SNAPSHOTS`)

- **New test aid:** with `PATORRENT_SNAPSHOTS=<folder>`, the GTK GUI draws each of its windows into `window-<n>.png` there every 2 s (GTK renders them itself) and lists their titles and label texts in `windows.txt`, dialogs included. It is for machines whose screen cannot be captured or read over SSH. On the Mac VM it gave the main window, the "folder in use" message and its text. Under Xvfb on Linux it gave the "Create it?" question as a clean picture with its text. On the slow VM the first files come about 15 s after the start. Documented in RELEASING.md.
- **Correction (2026-11-03_231320_1793744000):** the Mac VM's `screencapture` showing only the menu bar is not the missing GPU. The owner's TightVNC to the VM's Screen Sharing gives a perfect picture. It is macOS hiding window contents from a program without Screen Recording permission (SSH runs as `/usr/libexec/sshd-keygen-wrapper`).
- Clippy for Linux and macOS clean.

## 2026-11-03_231320_1793744000: The 1.1.54 hold in the Flatpak

- The Flatpak, rebuilt from 1.1.54 with flatpak-builder on the Xubuntu VM, under headless GNOME 50. A fresh `fp` profile's first window was matched to its own `….profile_fp.desktop` (`Exec=flatpak run --command=patorrent-gtk … --profile fp %U`) and had PaTorrent's badge. Before the hold, a first start was grouped under the default app. Docs only.
- On the Mac VM, `screencapture` returns only the menu bar even when started by a LaunchAgent inside the logged-in session, so it is not SSH's permission. Most likely the VM has no GPU acceleration. The VMware window on the host shows the screen.

## 2026-11-03_230801_1793743681: Version 1.1.54: a profile's first window waits for GNOME

- **GTK:** when a named profile's desktop entry is written at its start (its first start, or after the program moved), under GNOME (`XDG_CURRENT_DESKTOP`) the window shows 6 s later. GNOME Shell reads a new entry about 5 s late, and a window shown sooner kept a generic badge in the overview (in the Flatpak, the default app's group). This was the owner's choice over leaving it.
- Checked under headless GNOME 50 on the Xubuntu VM: a fresh `work` profile's window got PaTorrent's badge and its own entry, where the build before still showed a gear. Started minimized, or on other desktops, nothing waits. Clippy for Linux and macOS clean; GTK tests pass.

## 2026-11-03_223519_1793741719: D42 on a real Mac: a login starting the agents, Open Anyway

On the owner's Intel macOS 15.8.2 VM (VMware), with 1.1.53 built there from MacPorts' GTK 4.22 (Quartz):

- **Real login:** both profiles' LaunchAgents in `~/Library/LaunchAgents`, as Preferences writes them, pointed at `/Applications/PaTorrent.app`. After a reboot and automatic login, launchd started `PaTorrent 1.1.53` and `PaTorrent 1.1.53 [work]`, each holding its own library's lock. The first reboot also showed the single-instance rule: macOS reopened a second copy of the app left running before it, which took the default profile's folder, so the agent's default start got the "folder in use" message.
- **Open Anyway:** a quarantined copy in `~/Downloads/PaTorrent` beside `patorrent.ini` was refused ("“PaTorrent” Not Opened"). Clicking Open Anyway in Privacy & Security, then in the "Open “PaTorrent”?" dialog, and the owner's password ran it from `AppTranslocation`. The quarantine became `00c3`, the flag CI set by hand. The app showed its notice ("macOS is running PaTorrent from a temporary copy, so a patorrent.ini beside PaTorrent.app is not seen. Move PaTorrent.app to the folder you want (or to Applications) with Finder and start it again.", read from the VMware window) and used `~/Library`, since the marker beside it cannot be seen from there.
- Found: bundling from MacPorts fails on its SVG loader (no header padding), and the bundler's signing pass failed on that VM (BACKLOG). Docs only.

## 2026-11-03_204931_1793735371: D42 gate 8: an exFAT stick

- `macos-gate8.yml` gained an exFAT disk image as the stick (macOS 15's FSKit exFAT). The GUI ran from it and took its lock and library there. nox added a torrent saved on it; mounted at another path, nox found its state and the data, seeding with no recheck. The whole job passed (run 41705583119).
- The first try flagged a recheck by mistake: "Checking" at 0 s is the resume data's own check. A real recheck logs "resume data did not match the files", which the job now looks for.
- Docs and CI only.

## 2026-11-03_203047_1793734247: Version 1.1.53: gate 8 on a macOS runner; two portable fixes

- **Gate 8 (D42), run on GitHub's macOS 15 runner by the new hand-run workflow `macos-gate8.yml`:**
  - **Portable:** a `PaTorrent.app` beside `patorrent.ini` ran the default profile and `[work]` side by side, kept both libraries in that folder and wrote nothing to `~/Library`.
  - **LaunchAgents:** the per-profile LaunchAgents, as Preferences writes them, were started by launchd, each with its own library.
  - **Translocation:** a quarantined copy opened where it was unpacked was translocated. Unapproved, Gatekeeper refused the ad-hoc-signed app ("Not Opened"). With the user-approved flag that Open Anyway sets, the GUI ran from `AppTranslocation`, showed the move-the-app notice and wrote nothing beside itself.
  - The first run hung 45 minutes on the Gatekeeper prompt; the step now has time limits. One run waited in vain for a runner.
- **Fix (CI, macOS):** nox beside `patorrent.ini` on macOS used `~/Library` (its path code checked macOS before portable mode). It is portable there now, as slice 7 meant.
- **Fix (CI, Windows):** a save path inside the portable folder was stored absolute when spelt another way than the program's own path (the runner's `C:\Users\RUNNER~1` temp folder). Paths are now also compared resolved on disk (links, short names, a folder not made yet). New test; CI green on all platforms.

## 2026-11-03_151244_1793715164: Version 1.1.52: the GTK AppImage on Wayland, window classes per profile

- **AppImage:** the release workflow drops linuxdeploy's GTK hook line `export GDK_BACKEND=x11`, so the GUI runs on Wayland without Xwayland. Under headless GNOME 50 with no X11, the AppImage's `vpn-1` window ran next to native default and `work` windows, each matched to its own entry. On X11 under Xvfb it still starts.
- **GTK:** the program name is the profile's application id, so X11's `WM_CLASS` matches its desktop entry (it was `patorrent-gtk`, `AppRun.wrapped` in an AppImage). The application name stays "PaTorrent".
- **Fix:** a second start while a new profile's "Create it?" question was up asked a second time. Now the first window is awaited (checked with and without the change under Xvfb).
- **Not changed:** the first start's generic overview badge in GNOME. The shell takes about 5 s to read a new entry, so waiting would cost more than it fixes (BACKLOG).
- Clippy for Linux and macOS (GTK) is clean; GTK and frontend tests pass on the Xubuntu VM.

## 2026-11-03_145919_1793714359: D42 on real desktops: GNOME, KDE, the Flatpak, an AppImage, rc.d as root

- **FreeBSD 15.1, rc.d as root:** `patorrent_nox_profiles="default work vpn-1"` started three daemons as `patorrent`, each with its own state, downloads, log, pid file and Web UI port (8080-8082; listen ports 6881, 48807, 58905). `reload work`, `stop work` / `start work`, stop-all and the boot path (`faststart` / `faststop`) all worked. Cleaned up afterwards.
- **A real AppImage (Xubuntu 26.04):** both AppImages built with the release workflow's pinned linuxdeploy. Beside `patorrent.ini`, nox kept its state there with and without `--profile`, through FUSE and extract-and-run; without the file it used XDG. The GTK one under Xvfb showed `[work]`, kept its state in the folder, and wrote an entry that starts the `.AppImage` file. Under X11 its window has `_GTK_APPLICATION_ID` = the profile's id but `WM_CLASS` `AppRun.wrapped`. linuxdeploy's GTK hook forces `GDK_BACKEND=x11`, so it needs Xwayland (BACKLOG).
- **GNOME Shell 50 (headless, Wayland):** the window tracker gave the default window `io.github.balls0fsteel.PaTorrent.desktop` and the `work` window `….profile_work.desktop`, both with PaTorrent's icon. The dash showed two PaTorrent icons. On a profile's first start the overview badged its window with a generic icon, as the window came before GNOME re-read the new entry. From the next start it was right.
- **KDE (KWin 6.6, Wayland):** KWin gave the windows `desktopFileName` = the default id and `….profile_work`, the names Plasma's task manager looks up. No screenshot: KWin cancels them in a virtual session.
- **Flatpak (GNOME 51 runtime, built by flatpak-builder from this tree):** `--profile fp` kept its folders under `~/.var/app`. It wrote `….profile_fp.desktop` to the host's `~/.local/share/applications`, with `Exec=flatpak run --command=patorrent-gtk … --profile fp %U`. From the second start GNOME matched the window to it, with the right icon. On the first start the window was grouped under the default Flatpak app.
- Docs only. Left: gate 8 on a Mac.
