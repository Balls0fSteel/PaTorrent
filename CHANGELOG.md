# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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

## 2026-11-03_132148_1793708508: Version 1.1.50: macOS portable mode (D42 slice 7)

- **New, not run on a Mac:** `patorrent.ini` in the folder holding `PaTorrent.app` (never inside the bundle, which would break its signature) makes it portable; a bare binary (nox) is portable beside itself, as on Linux. A quarantined app that macOS runs translocated (from a random read-only copy, where the marker is not seen) says so once its window is up and asks to move `PaTorrent.app` with Finder (`MacTranslocated`, 34 machine translations). `profile::app_bundle_parent`, `translocated`.
- **Checked:** unit tests for the bundle and translocation paths; 1,549 workspace tests; clippy for four targets (GTK on macOS too). Open: a run on a real Mac (gate 8).

## 2026-11-03_131614_1793708174: Version 1.1.49: portable mode everywhere, relative paths (D42 slice 6)

- **Portable on Linux/BSD:** `patorrent.ini` beside the binary (or beside the `.AppImage`; never in a Flatpak) makes its folder the root there too: the GTK client keeps settings and library in it (a profile in `profiles/<name>`), nox its `nox` folder (the flags and systemd still win). A portable folder that cannot be written is an error (`PortableReadOnly` in the GUIs, 34 machine translations; nox's `error:`), never a fall-back to the user's folders. macOS stays without portable mode (slice 7).
- **Relative paths:** in portable mode the paths inside the folder are written relative to it (`./Downloads`, `/` separators) and resolved at load: torrents' save paths and absolute renamed files in `resume.db` (`SessionParams.portable_root` -> `Store::open_in`), Preferences' folders, the IP filter file, the Add dialog's recent folders and awaiting magnets' folders (`Prefs`, `profile::is_path_key`), the RSS filters' save folders (`rss.dat`). In memory every path stays absolute; paths outside the folder stay absolute; existing absolute ones inside it become relative at their next save. New `patorrent_core::util::portable`.
- **First start of a portable folder:** downloads go to its `Downloads` (and a random port, 1.1.45). The update offer treats a portable copy inside an install folder as portable (the release page, not the setup).
- **Downgrade note:** a portable folder written by this version needs it or later: 1.1.48 and earlier would read `./Downloads` against their working directory.
- **Checked:** unit tests (the path rules, UNC and other drives, `..` refused, the store, `Prefs`, `rss.dat`, the candidate folder), a nox integration test (a copied binary with `patorrent.ini`: state in `nox`, `resume` holds `./Downloads`, the folder renamed and started from elsewhere finds its data and seeds), 1,549 workspace tests, clippy for four targets; a fresh portable `patorrent.exe` wrote `dir.download=./Downloads`. Left out: the Web UI's categories (`webui.json`). Not run: Linux portable (tarball, AppImage), the Samba move.

## 2026-11-03_130347_1793707427: Version 1.1.48: File > Profiles (D42 slice 5)

- **New:** File > Profiles in both GUIs: the default profile and every named one (this one checked; Win32 refills it each time it opens, so a profile made elsewhere shows), each starting its own process (`--profile`; a running one comes to the front); New Profile (a name box, validated, makes the folders and starts it); Create Shortcut (Windows: `PaTorrent (work).lnk` on the desktop through the shell's `ShellLink`, declared by hand in `shortcut.rs` as windows-sys has no COM interfaces; Linux/BSD: the named profile's desktop entry joins the applications menu, greyed for the default profile and on macOS); Open Profile Folder. 9 strings, machine-translated into the 34 tables.
- **Checked:** the `.lnk` test (writes and reads back a shortcut with `--profile work`), 1,540 workspace tests, clippy for four targets; File > Profiles captured on the hidden desktop (Default, vpn-1 made by hand, work checked and greyed). GTK not run on a desktop.

## 2026-11-03_125404_1793706844: Version 1.1.47: nox services per profile, the installer's checks (D42 slice 4)

- **Windows service:** `service install --profile work` registers `PaTorrentNox_work` ("PaTorrent nox (work)", state `%ProgramData%\PaTorrent\profiles\work\nox`, `--profile work` in the image path); `uninstall`, `start` and `stop` take `--profile`. New `service status [--profile NAME | --all [--in DIR]]` (state and image path, exit 0 listed / 4 none; no elevation) and `service uninstall --all [--in DIR]`. The running service registers under its profile's name.
- **systemd / FreeBSD:** `contrib/systemd/{system,user}/patorrent-nox@.service` templates (`run --profile %i`, `StateDirectory=patorrent-nox/%i` for the system one, no default `--webui`); the rc.d script has rc.subr profiles (`patorrent_nox_profiles`, `patorrent_nox_<p>_dir/_save/_log/_args`, a pid file each).
- **Installer:** the uninstaller asks `patorrent-nox.exe service status --all --in "$INSTDIR"` (nsExec, no console) instead of reading `PaTorrentNox`'s ImagePath, so a profile's service is found too, and its hint says `service uninstall --all`; the setup refuses a folder holding `patorrent.ini` (`PT_PORTABLE_DIR`, 35 languages; the Directory page and silent installs); "remove settings" also removes `profiles\` and `patorrent.lock`.
- DAEMON.md: a Profiles section, the templates, rc.d profiles, the service's profile options.
- **Checked:** unit tests (the choice of services, image paths, the profile's default state); 1,539 workspace tests; clippy for four targets; `sh -n` on the rc.d script; makensis builds the setup; `service status --all` / `--profile work` against this PC's SCM (exit 4, none installed). Not run: an elevated install of a profile's service, systemd on the Xubuntu VM, the setup on the Windows VM.
