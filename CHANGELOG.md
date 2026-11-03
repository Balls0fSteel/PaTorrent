# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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

## 2026-11-03_140518_1793711118: D42: AppImage rule, Create Shortcut, an unfinished torrent moved

- **Beside an AppImage (Xubuntu VM):** with `$APPIMAGE` naming a `.AppImage` file beside `patorrent.ini` and the binary elsewhere (as in the AppImage's mount), nox kept its state beside the `.AppImage`. A real AppImage was not built (that needs `linuxdeploy` downloaded).
- **Create Shortcut (Windows VM, console session):** File > Profiles > Create Shortcut in a `--profile work` window made `PaTorrent (work).lnk` on the desktop, target the exe, arguments `--profile work`.
- **An unfinished torrent moved:** a portable folder with a torrent at 50 % moved to another drive letter resumed at 50 % Downloading, although the missing half had been filled in behind its back: no recheck.
- Docs only. Still open: a Mac (gate 8); dock icons under GNOME, KDE and the Flatpak; the rc.d profiles started as root.

## 2026-11-03_140153_1793710913: D42 desktop checks: the wizard, double-clicks, the Samba move

- **Windows VM, the console session** (`tools/windows-clean/desktop/wizard-profiles.ps1`, new): the setup wizard given a folder holding `patorrent.ini` shows `PT_PORTABLE_DIR`, stays on its Directory page and installs nothing; the interactive uninstaller with "Remove settings" ticked removes `%APPDATA%\PaTorrent` with `profiles\` and `patorrent.lock` (unticked, they stay). UIA sees only panes on a locked desktop, so the folder box is set with `WM_SETTEXT` and the box ticked with a posted Space on the components tree.
- **Windows VM:** a `.torrent` opened through the shell (as a double-click) with the association held by profile `work` started `patorrent.exe --profile work --shell ...`; a second one went to that window; `work` saved the torrent. An installed and a portable copy running at once each got only their own second start's torrent.
- **This PC:** a click on "Port 45200 in use" opened Preferences at Connection.
- **The Samba move:** a portable nox folder with a finished 100 MB torrent, made here, copied to the VM's share, run there by the Linux binary from `/srv/media`, then copied back and run from another drive letter (`subst P:`): both found it at 100 % without a recheck (proved by changing the file's bytes but not its size and time: still 100 % Seeding). Windows does not run programs from that share (access denied), hence the copy back.
- Docs and scripts only, no version bump. Still open: a Mac (gate 8), GNOME/KDE dock icons and the Flatpak, beside an AppImage, rc.d started as root.

## 2026-11-03_134710_1793710030: Version 1.1.51: the D42 checks on the VMs, two GTK fixes

- **Windows VM (clean, 1.1.50's zip and setup built here):** `zip-test.ps1` all ok, a profile beside the default one (its own port, `./Downloads`), a portable nox writing `./Downloads`, no Defender detections; `setup-test.ps1 -OldVersion 1.1.40` 0 failures, among them `PaTorrentNox_x` holding the silent uninstall until `service uninstall --all` and a portable target refused. `service install --profile work --now` under the real SCM: `PaTorrentNox_work` running, state in `%ProgramData%\PaTorrent\profiles\work\nox`, its own port, stopped and removed; the VM left clean.
- **This PC, hidden desktop:** two portable copies at once; a second start of one with a `.torrent` added it to that copy's window only (the other showed "Port 6881 in use", both kept the default port).
- **Xubuntu VM (Xvfb, a private D-Bus):** 1,465 Linux tests and the portable nox test; nox `--profile` folders and name-derived port, the lock between two processes, a bad name, `systemctl --user start patorrent-nox@d42test` running. GTK: a bad name's message, the creation question (nothing made before the answer), `[d42gtk]` in the title, a second start reaching the first, the default profile running beside it, the hidden desktop entry, File > Profiles (the current profile checked) starting `vpn-1` in its own window, Create Shortcut putting `PaTorrent (vpn-1)` in the menu, portable mode beside a copied binary (`./Downloads`, files beside it), a read-only portable folder's message. **FreeBSD VM:** the rc.d script's profile variables traced with `sh -x` (no root there to start it).
- **Fixed (GTK):** SIGTERM did nothing while a start-up error or question was up (no window state to close): the application now quits. The read-only and folder-in-use messages were one bold heading: the first paragraph is the heading now, the rest the text.
- Still open: a Mac (gate 8), dock icons under GNOME/KDE and the Flatpak, the Samba move, a double-clicked `.torrent` on a real desktop.

## 2026-11-03_132346_1793708626: Profiles and portable mode: docs and the VM scripts (D42 slice 8)

- RELEASING.md: the profile, association, portable, service and downgrade checks for a draft. DAEMON.md: portable on every platform with relative paths. PROFILES.md: implemented. BACKLOG: a "Profiles and portable mode (D42)" section with every open hand check (Windows VM, GTK on Linux, the Samba move, macOS) and what was left out.
- `tools/windows-clean`: `zip-test.ps1` starts a profile beside the default one and checks a portable nox writes `./Downloads`; `setup-test.ps1` registers `PaTorrentNox_x` and expects the silent uninstall to refuse until `service uninstall --all`, expects a portable target folder refused, and keeps `profiles\` on a silent uninstall. Both parse; not run on the VM yet.
- **Website:** the FAQ's settings answer and the download page's settings lines are on the branch `site-profiles`, to merge when the release with profiles is published (a push to `main` that changes `site/` deploys at once, RELEASING.md step 2).
- Docs and scripts only, no version bump.
