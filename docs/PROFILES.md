# Profiles and portable mode (V2.0 design)

Status 2026-10-30_090053_1793347253: **proposal, proposed D42** ([DECISIONS.md](DECISIONS.md)). Nothing here is implemented; no code changes until the owner approves. V2.0 goals 1 and 3 in [PLAN.md](../PLAN.md); it revisits D7 and touches D16, D22, D31 and D40.

## Goals

1. **Profiles:** several independent PaTorrents for one user: separate settings, torrents and state, running at the same time, each with its own single-instance handling, ports and (for nox) service.
2. **Portable mode on every platform:** settings, state and, by default, downloads in one folder that can move (a USB stick, a synced folder), with paths inside it kept relative.

Non-goals: sharing torrents between profiles, switching profile inside a running window, multi-user (per-account) access to one profile, profiles inside the Flatpak's autostart (see open questions).

## Where things live today

| Binary | Default folders | Portable | Single instance |
|---|---|---|---|
| `patorrent` (Win32) | `%APPDATA%\PaTorrent` (`base_data_dir()`, `crates/win32/src/app.rs`); GUI state in `state\`; remote windows in `remote\<host>_<port>` | `patorrent.ini` beside the exe, tested with `exists()` (D7) | Mutex `Local\PaTorrent-<data dir>` (`crates/win32/src/main.rs`, `instance::forward`), then `FindWindowW(MAIN_CLASS)` with `MAIN_CLASS = "PaTorrentMainWnd"`, **not keyed by data dir** |
| `patorrent-gtk` | config `$XDG_CONFIG_HOME/patorrent/gtk`, data `$XDG_DATA_HOME/patorrent/gtk`; macOS both `~/Library/Application Support/PaTorrent/gtk` (`crates/gtk/src/paths.rs`) | none | GApplication uniqueness on `APP_ID` (`io.github.balls0fsteel.PaTorrent`), which is also the icon, desktop entry, tray and LaunchAgent name; remote windows are `NON_UNIQUE` |
| `patorrent-nox` | state `$XDG_DATA_HOME/patorrent/nox`, config `$XDG_CONFIG_HOME/patorrent/nox` (systemd's `$STATE_DIRECTORY` / `$CONFIGURATION_DIRECTORY` first); Windows `%APPDATA%\PaTorrent\nox`; macOS `~/Library/Application Support/PaTorrent/nox`; `--state-dir` / `--config-dir` override (`crates/nox/src/paths.rs`) | Windows only: `nox\` beside the exe when `patorrent.ini` is there, tested with **`is_file()`** (`portable_dir()`) | none; `--pid-file` locks a pid file only when given |

Fixed names and ports: listen `0.0.0.0:6881,[::]:6881` (`crates/core/src/settings.rs`); hosted Web UI 8080 (`crates/win32/src/webui_host.rs`, D40) and nox's documented `--webui 127.0.0.1:8080` (`contrib/systemd/*`); Windows service `PaTorrentNox` (`crates/nox/src/service.rs`, checked by name in `packaging/windows/patorrent.nsi`); `contrib/systemd/{system,user}/patorrent-nox.service`; `contrib/freebsd/patorrent_nox` (rc.d, `patorrent_nox_dir`); Win32 ProgIDs `PaTorrent.torrent` / `PaTorrent.magnet` and the `Run` value `PaTorrent` (`crates/win32/src/integration.rs`).

Existing problems this design fixes on the way:

- **Window lookup is not keyed.** A portable copy and an installed copy may run together (the mutex is per folder), but a second start of either forwards its torrent to whichever `PaTorrentMainWnd` `FindWindowW` finds first, possibly the other copy's. This is a 1.x bug that can be fixed before V2.0 (step 1 below).
- **Two portable tests.** A directory named `patorrent.ini` makes the GUI portable but not nox. One shared test, `is_file()`, in `patorrent_frontend` (nox already depends on it).
- **Nothing but the GUI mutex and D-Bus guards the files.** Two GTK sessions on one home (a second login, `ssh -X`), two nox processes on one state directory, or one portable folder opened from two PCs over a share can open one `resume.db` twice. A lock file in each profile folder becomes the real guard (step 2).

## Design

### A profile is a whole PaTorrent root

The **default profile** is today's layout, unchanged: no migration, and 1.x and 2.0 read the same files. A **named profile** inserts `profiles/<name>` between the application root and the per-program folder, so a profile looks exactly like a fresh default install:

| | default profile | profile `work` |
|---|---|---|
| Win32 GUI | `%APPDATA%\PaTorrent\` | `%APPDATA%\PaTorrent\profiles\work\` |
| nox, Windows | `%APPDATA%\PaTorrent\nox\` | `%APPDATA%\PaTorrent\profiles\work\nox\` |
| GTK config / data | `~/.config/patorrent/gtk`, `~/.local/share/patorrent/gtk` | `~/.config/patorrent/profiles/work/gtk`, `~/.local/share/patorrent/profiles/work/gtk` |
| nox, Linux/BSD | `~/.local/share/patorrent/nox`, `~/.config/patorrent/nox` | `.../patorrent/profiles/work/nox` in both |
| macOS (GTK, nox) | `~/Library/Application Support/PaTorrent/{gtk,nox}` | `.../PaTorrent/profiles/work/{gtk,nox}` |
| portable folder `P` | `P\` (and `P\nox\`) | `P\profiles\work\` (and `...\nox\`) |

Remote windows keep their `remote/<host>_<port>` folders inside the profile they were opened from. A GUI profile and a nox profile with the same name are unrelated engines that only share a folder.

**Names:** 1-32 characters from `a-z`, `0-9` and `-`, starting with a letter, compared without case and stored lower case. `default` means the default profile. The narrow set keeps the name usable unchanged in folder names on every file system, Windows service names, systemd instance names, rc.d variable names (with `-` as `_`) and, with `-` as `_`, a D-Bus name element (below). Anything else is refused with a message, not rewritten.

### Choosing a profile

- `--profile NAME` on all three binaries; `--profile default` (or no flag) is the default profile. It may come anywhere among the arguments and is never forwarded as a source.
- **GUIs:** a profile that does not exist yet is created on first use after a yes/no question (a typo should not silently start an empty client). The title bar and tray tooltip show it: `PaTorrent 2.0.0 [work]`; nothing changes for the default profile.
- **nox:** creates the folder without asking, as it creates its state directory today. `--profile` only changes the *defaults*: `--state-dir` / `--config-dir` and systemd's directories still win, as they beat XDG today; the profile name still names the service and the log lines.
- Shortcuts are the user's: `patorrent.exe --profile work`. A **File > Profiles** submenu (list, "New profile...", each item starting another process) is an option for the owner (Q3).

### Single instance per profile

**Win32.** The mutex is already keyed by `data_dir()`, so it follows the profile. The window class gets the same key: `PaTorrentMainWnd` stays for the default, non-portable folder (so a 1.x second start still finds a 2.0 window), every other folder (a profile, a portable copy) registers `PaTorrentMainWnd.<16 hex digits of FNV-1a 64 of the lower-cased data dir>`, and `instance::forward` looks for that class. `REMOTE_CLASS` and the SEC-25 integrity filter stay as they are. Optional: a per-profile `AppUserModelID` (`PaTorrent.work`), so the taskbar groups and pins profiles apart.

**GTK.** `application_id` becomes `APP_ID` for the default profile and `APP_ID.profile_<name with - as _>` for a named one (for example `io.github.balls0fsteel.PaTorrent.profile_work`); the uniqueness, `command-line` forwarding, macOS `odoc` / `GURL` handling and `GNotification` all follow from the id. `APP_ID` stays the constant for the icon, desktop entry, tray `Id` and LaunchAgent, so `application_id` gets its own function. The cost: on Wayland the window's `app_id` is the application id, and the shell matches it to a desktop entry. A named profile therefore writes a hidden entry (`NoDisplay=true`, `Icon=` the base `APP_ID`, `Exec=... --profile <name> %U`) beside its other integration files on first start, so the dock shows the right icon and name. Inside the Flatpak the sandbox may own names below its own app id (believed, to be checked) but cannot export new desktop entries, so profile windows there may show a generic icon (Q6).

**Lock file (all binaries).** Each profile folder (and nox's state directory) holds `patorrent.lock`, locked exclusively for the process's life (`LockFileEx` through `windows-sys`; `flock` through a hand-written declaration on Unix, as the TLS code does: `File::try_lock` is newer than MSRV 1.85). A second process that loses the race on the mutex / D-Bus name still finds the lock taken and exits with a message instead of opening the store. nox's `--pid-file` keeps its meaning.

### Ports

Every profile defaults to the same ports today, and the second one fails to listen (`Alert::ListenFailed`) or to start its Web UI.

- **Listen port:** a **new** profile (and a fresh portable folder) writes a random listen port in 10000-60000 into its settings on creation, as µTorrent picked one at first run. The default profile keeps 6881 where it already has it. A listen failure shows in the status bar (not only the log) with a link to Preferences > Connection.
- **Web UI:** 8080 stays the default; when the user enables the Web UI in a profile, the page proposes the first free port from 8080 up, and a failure to bind says which port and that another profile may hold it.
- **UPnP / NAT-PMP:** each instance maps its own port; the mapping description names the profile (`PaTorrent (work)`) so routers' tables tell them apart.
- The engine refuses nothing: two profiles may be set to one port on purpose (different interfaces); only the defaults avoid clashes.

### nox services

| Service manager | default profile | profile `work` |
|---|---|---|
| Windows SCM (`service install --profile work`) | `PaTorrentNox`, "PaTorrent nox" | `PaTorrentNox_work`, "PaTorrent nox (work)"; image path carries `--profile work` |
| systemd (`contrib/systemd/{system,user}`) | `patorrent-nox.service` | new template `patorrent-nox@.service` (`systemctl enable --now patorrent-nox@work`): `StateDirectory=patorrent-nox/%i`, `run --profile %i`, and no default `--webui` port, so the user sets one per instance in a drop-in |
| FreeBSD rc.d (`contrib/freebsd/patorrent_nox`) | `patorrent_nox_enable` | rc.subr's profiles convention: `patorrent_nox_profiles="work"`, `patorrent_nox_work_dir`, `..._work_save`, `..._work_args`, one pid file each |
| macOS launchd (if shipped) | `<APP_ID>.nox` | `<APP_ID>.nox.work` |

`service` subcommands (`install`, `start`, `stop`, `remove`, `status`) take `--profile`. The installer's upgrade and uninstall checks, which look for `PaTorrentNox`'s image path today, enumerate every `PaTorrentNox` and `PaTorrentNox_*` service whose image is in `$INSTDIR`. D40's hosted Web UI is per window and therefore already per profile.

### Associations and autostart

Only one program can own `.torrent` and `magnet:` per user, so associations belong to one profile at a time.

- **Windows:** ticking the association in a profile's Preferences writes the ProgIDs' command as `"<exe>" --profile <name> "%1"` (default: as today). The "is it ours" test (`runs(&c, exe)`) compares the profile as well, so only the owning profile shows the box ticked; ticking it in another profile takes it over. The `Run` value is per profile (`PaTorrent`, `PaTorrent (work)`), so profiles autostart independently. `--associate` (D31's installer) writes the default profile only; `--unregister` removes every association and `Run` value that names this exe, whatever the profile.
- **Linux / BSD:** the MIME default stays the one desktop entry; a profile that takes the association writes its hidden entry as the handler. Autostart entries are per profile (`<APP_ID>.profile_work.desktop` in `~/.config/autostart`). The Flatpak's Background portal keeps one autostart per app, so there only the default profile can autostart (Q6).
- **macOS:** Launch Services picks the bundle, not an argument, so file and link opening always reach the default profile; per-profile LaunchAgents (`<APP_ID>.work.plist`) handle login start.

### Portable mode

**Marker.** `patorrent.ini` as a **file** in the portable root, the D7 rule extended to every platform, tested once in `patorrent_frontend` for all three binaries. The portable layout is the same everywhere: the GUI's files in the root (as the Win32 GUI already does; GTK's config and data names already coexist in one folder on macOS), `nox/`, `remote/`, `profiles/<name>/`.

| Platform | Portable root |
|---|---|
| Windows zip | the exe's folder (unchanged) |
| Linux / BSD tarball | the binary's folder |
| AppImage | the folder **holding the `.AppImage`** (`$APPIMAGE`'s parent; `current_exe` points into the read-only mount). AppImage's own `<name>.AppImage.home` / `.config` folders keep working through XDG, without the relative paths below. |
| Flatpak | none: the sandbox cannot see beside itself; `--profile` covers multiple setups there |
| macOS | the folder **holding `PaTorrent.app`** (three levels above `Contents/MacOS/`), never inside the bundle (that breaks its signature). A quarantined app downloaded from the web runs translocated from a random read-only path, where the marker is not seen: the app detects an `AppTranslocation` path and tells the user to move it. Lowest priority (Q8). |

**Unwritable root.** A portable root that cannot be written (Program Files, a read-only stick, a mounted dmg) is an error with a message, never a silent fall-back to `%APPDATA%`, which would split one user's data in two.

**Relative paths.** In portable mode every stored path that lies inside the portable root is written relative to it (`./Downloads/...`, `/` separators) and resolved at load: torrent save paths in `resume.db`, the default and "move completed" folders, the auto-load folder, RSS save paths, the IP filter and Web UI certificate paths. Paths outside the root stay absolute. On first start in portable mode the default download folder is `<root>/Downloads`. Existing absolute paths inside the root are rewritten relative on the next save, so making a folder portable converts it. Outside portable mode nothing changes. This is the one change that touches the store format's contents (not its layout); the engine keeps absolute paths in memory and only the frontend's load/save converts.

**Integration in portable mode.** A portable copy never writes associations, autostart or services unless the user asks in Preferences, as today; when it does, those name the portable exe and break if the stick moves, which Preferences says next to the box. The update notice (D32) offers the zip, not the setup, to a portable copy.

### The installer (D31)

D31 holds: the setup installs without `patorrent.ini`, so installed copies use `%APPDATA%\PaTorrent` and its profiles. Additions:

- The setup refuses a target folder that holds `patorrent.ini` (someone's portable copy) rather than turning it into an installed one.
- The uninstaller already deletes its own files one by one and removes `$INSTDIR` only when empty, so a portable folder made inside the install folder survives; "remove settings" also removes `profiles\` in `%APPDATA%\PaTorrent`, and every `PaTorrentNox_*` service is removed first (as `PaTorrentNox` is).
- Start menu: one "PaTorrent" shortcut; profile shortcuts are the user's (or File > Profiles creates them, Q3).

### Migration and compatibility

- 1.x to 2.0: nothing moves; the default profile is the old folder. 2.0 back to 1.x: 1.x ignores `profiles/` and `patorrent.lock`; a portable folder converted to relative paths would need 2.0 (1.x would read `./Downloads` as relative to its working directory). Converting only on write, with a one-line warning in the changelog, keeps that acceptable; alternatively keep a `paths.relative=1` key so 1.x users notice (Q7).
- Moving between installed and portable stays a copy of the folder; a "copy this profile to a portable folder" command is a possible later addition, not V2.0.
- The Win32 window class for the default folder is unchanged, so mixed 1.x/2.0 starts still forward.
- nox's existing `paths::migrate` (`webui.json` to the config directory) runs per profile unchanged.

## Order of work (after approval)

1. Key the Win32 window class by data dir; unify the portable test (`is_file()`, one function). Fixes the 1.x bug; small; can ship in 1.1.x.
2. `patorrent.lock` in all three binaries.
3. `--profile` and the folder rule in a shared `patorrent_frontend::profile` module (name validation, root resolution, portable root), used by all three; title, window class, GTK id.
4. Ports for new profiles; status-bar listen failure; Web UI port proposal.
5. Associations, autostart and the hidden GTK entry per profile.
6. nox services: `--profile` on `service`, the systemd template, rc.d profiles, the installer's service checks.
7. Portable mode on Linux/BSD (tarball, AppImage) and the relative paths; then macOS.
8. Docs: user guide pages, DAEMON.md, RELEASING.md checks; hand checks on each platform.

Each step bumps the patch version (D37); none changes wire behaviour, so `PEER_ID_PREFIX` stays.

## How others do it (from memory; not re-verified)

- **µTorrent 2.2.1:** portable when `settings.dat` is beside the exe, else `%APPDATA%\uTorrent`; no profiles. People ran several copies from separate folders; whether 2.2.1's single-instance check allowed that without renaming the exe is reported both ways. Picks a random listen port at first run. Stores absolute paths in `resume.dat` (so drive letters break portable copies), as far as I recall.
- **qBittorrent:** `--profile=<dir>` moves the whole configuration root; `--configuration=<name>` appends `_<name>` to the folder names, giving named configurations side by side; on Windows a `profile` folder beside the exe makes it portable; `--relative-fastresume` stores save paths relative to the profile. Its single-instance key is believed to include the profile path and configuration name, so instances with different ones run together. PaTorrent's `--profile NAME` corresponds to `--configuration`, and the portable marker to the `profile` folder.
- **Transmission / Deluge:** only a configuration directory flag (`-g` / `-c`); multiple daemons need distinct directories and ports chosen by hand.

## Open questions for the owner

- **Q1** Approve the folder rule (`profiles/<name>` inside the root, default profile = today's folder)?
- **Q2** Name rule `[a-z][a-z0-9-]{0,31}`, or allow Unicode names (at the cost of encoding them for services and D-Bus)?
- **Q3** Is the command line enough for V2.0, or also a File > Profiles submenu (and a "create shortcut" item on Windows)? A `PATORRENT_PROFILE` environment variable as well?
- **Q4** Random listen port for new profiles (µTorrent-like), or 6881 + n?
- **Q5** Associations owned by one profile at a time (proposed), or always the default profile?
- **Q6** Flatpak: accept generic icons for profile windows and default-only autostart, or leave profiles out of the Flatpak?
- **Q7** Relative paths in portable mode: convert existing paths inside the root on save (proposed), and how much 1.x downgrade compatibility matters.
- **Q8** macOS portable mode in V2.0, or later (translocation and the Mac validation gap, gate 8)?
- **Q9** Ship step 1 (keyed window class, one portable test) in 1.1.x ahead of V2.0?
