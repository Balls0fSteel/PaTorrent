# Profiles and portable mode: implementation plan (D42)

Written 2026-11-03_115032_1793703032 from the owner's request for the best approach by use case and a plan to implement. It answers [PROFILES.md](PROFILES.md)'s Q1-Q9 and replaces its "Order of work"; the design there still holds where this plan does not say otherwise. D42 is approved with this plan ([DECISIONS.md](DECISIONS.md)); each slice is one change set with a patch bump (D37). None changes wire behaviour, so `PEER_ID_PREFIX` stays. Whether the finished set is released as 2.0.0 or 1.2.x is the owner's call at release time.

## What the code showed

Mapped before planning; five findings change the design:

1. **`resume.db` belongs to the engine.** `Store::open` / `Store::put` and the record codec (`ResumeData::to_entry` / `from_node`, `crates/core/src/resume.rs`) live in core; `SessionParams` (`crates/core/src/session.rs`) has only `settings` and `state_dir`. Relative save paths are converted in core, with the portable root passed in, not in the frontend's load/save as PROFILES.md said.
2. **`--profile NAME` would add `NAME` as a torrent today.** The Win32 parser (`crates/win32/src/app.rs`, `parse_args`) and the GTK `command-line` handler drop unknown `--x` flags but keep the next word as a source; nox (`crates/nox/src/main.rs`, `parse`) errors on unknown options. All three parsers change.
3. **Remote windows relaunch the exe with only `--remote`** (`crates/win32/src/remote_ui.rs`, `crates/gtk/src/remote.rs`), so they must pass `--profile` along.
4. **nox has no `service status`** (`crates/nox/src/service.rs`: install, uninstall, start, stop, run, debug), and the installer checks the service only in the uninstaller, by ImagePath prefix (`packaging/windows/patorrent.nsi`); upgrades only test whether `patorrent-nox.exe` is in use.
5. **The Flatpak has `--filesystem=home`**, so a named profile can write its desktop entry to `~/.local/share/applications` and the dock gets the right icon; only autostart (Background portal, one per app) stays default-only.

Also: `random_port()` exists (`crates/frontend/src/options.rs`, 10000-65000); the Win32 status bar has a clickable notice part already (the update notice); nox's `PidFile` (`crates/nox/src/daemon.rs`) is the lock pattern (Windows share mode, Unix `flock` by hand-written declaration); `updater::installed()` decides "portable" from the Uninstall key, not from `patorrent.ini`.

## Use cases and what serves them

| # | Use case | What serves it | Slice |
|---|---|---|---|
| U1 | A VPN-bound client beside a normal one: different listen interface, proxy, ports, labels, RSS | A named profile is a whole separate root (`profiles/<name>`) with its own port by default and its own settings; both run at once | 1, 2 |
| U2 | Private trackers apart from public ones (ratio, seeding goals, Torznab sources) | Same; the association can be given to either profile | 1, 3 |
| U3 | Trying a new version or a setting without touching the real setup (also our own testing) | `--profile test`; the GUI asks before creating a profile, so a typo does not start an empty client | 1 |
| U4 | A USB stick, a synced folder, moving to a new PC | A portable root on every platform, downloads in `<root>/Downloads`, paths inside the root stored relative | 6, 7 |
| U5 | A server with several daemons (per purpose or per person), each with its own Web UI | nox `--profile`, per-profile services (SCM, systemd template, rc.d profiles), separate state and config | 4 |
| U6 | Two copies started by accident, or one folder opened from two PCs over a share | The window class keyed by folder; `patorrent.lock` in every profile | 0 |
| U7 | Finding and starting profiles without a command line | File > Profiles: list, New profile, Create shortcut, Open folder | 5 |
| U8 | The Store build (proposed D46) | Profiles work (`%APPDATA%` is virtualised per package); portable mode is ignored there, as STORE.md says | 1 |

Non-goals as in PROFILES.md: no sharing between profiles, no switching inside a running window, no multi-user access to one profile.

## Decisions (PROFILES.md Q1-Q9)

- **Q1 Folder rule: as proposed.** `profiles/<name>` inside the application root; the default profile is today's folder, nothing moves, 1.x and 2.0 read the same files. A profile looks like a fresh install (`<profile>/` for the GUI, `<profile>/nox/` for nox, `<profile>/remote/<host>_<port>` for remote windows).
- **Q2 Names: `[a-z][a-z0-9-]{0,31}`**, compared without case, stored lower case; `default` is the default profile. Anything else is refused with a message. No Unicode (it would need encoding for services, D-Bus and rc.d).
- **Q3 The command line and a File > Profiles submenu**, both GUIs, as the last GUI slice (U7): the command line is the mechanism, the menu is what makes it a feature. No `PATORRENT_PROFILE` environment variable (one way to choose a profile; shortcuts, desktop entries and services all carry arguments).
- **Q4 Ports: a random listen port (10000-60000, the existing `random_port()` narrowed) for every new named profile and every fresh portable root**, written into its `patorrent.ini` at creation. The default profile keeps 6881 (documented, firewall rules exist). `6881 + n` is rejected: it depends on creation order and collides after a profile is deleted and another created.
- **Q5 Associations belong to one profile at a time** (ticking the box in a profile takes them over); the installer's `--associate` writes the default profile. Always-default would make U1/U2's main profile unreachable from a double-click.
- **Q6 Flatpak: profiles are supported.** The sandbox may own `<APP_ID>.*` bus names and `--filesystem=home` lets it write the profile's desktop entry, so the dock shows the right icon (to verify in slice 3). Only autostart stays default-only. No portable mode in the Flatpak.
- **Q7 Relative paths: convert on write, in portable mode only.** Paths inside the root are written `./...` with `/` separators and resolved at load; the engine keeps absolute paths in memory. One changelog line warns that a portable folder written by 2.0 needs 2.0 (1.x would read `./Downloads` against its working directory). No `paths.relative` key: it would only tell 1.x users something 1.x cannot act on.
- **Q8 macOS portable mode last and optional** (slice 7): translocation handling and no Mac to validate (gate 8). Profiles and per-profile LaunchAgents on macOS come with slices 1 and 3.
- **Q9 Yes: slice 0 ships in 1.1.x** ahead of the rest. It is a bug fix (a second start can hand a torrent to the other copy; nothing guards a folder opened twice).

Refinements beyond PROFILES.md, from the code:

- **The lock needs no new `windows-sys` features.** `patorrent.lock` is opened with share mode 0 on Windows (`std::os::windows::fs::OpenOptionsExt::share_mode`, which also holds over SMB) and `flock(LOCK_EX | LOCK_NB)` on Unix, as nox's `PidFile` does. One helper in `patorrent_core::util` (beside `private_file`), used by all three binaries. Order in the GUIs: mutex / D-Bus first (forward the sources to the running window), then the lock; a free mutex with a taken lock means another computer or session: a message box and exit. nox: `--pid-file` keeps its meaning; the lock is taken in addition.
- **Relative paths in core:** `SessionParams` gains `portable_root: Option<PathBuf>`; `Store::open` / `put` pass it to the record codec, which converts `save_path` (and `mapped_files` values) both ways. The frontend converts its own keys (`dir.download`, `dir.completed`, `dir.torrents`, `dir.finished_torrents`, `dir.autoload_dir`, `ipfilter.path`, `add.recent_dirs`, `torrent.<key>.awaiting`, Web UI categories, RSS `save_in`) through one helper pair in `frontend::profile`. `lt.drive_overrides` keys are drive letters and stay as they are.
- **The UPnP description** comes from a new `SessionParams.label` (`"PaTorrent (work)"`), not from `settings.user_agent`, which is also the tracker User-Agent.
- **The Windows open command becomes `"<exe>" --profile <name> --shell "%1"`**: `--profile` must precede `--shell`, which stops parsing (SEC-18).
- **nox gains `service status [--profile NAME | --all]`**, printing state and image path, with an exit code the installer can use (`ExecWait` in NSIS): that replaces the ImagePath prefix check and covers `PaTorrentNox_*`.
- **The installer's portable test:** `updater::installed()` also returns `None` when `patorrent.ini` is a file beside the exe, so a portable copy inside an install folder is offered the release page, not the setup.

## Slices

Each is one change set with a patch bump; `cargo test --workspace`, clippy and rustfmt green on all CI targets before it is pushed.

### Slice 0: the 1.x fixes (ships in 1.1.x)

- `crates/frontend/src/profile.rs` (new, first part): `portable_root() -> Option<PathBuf>` with the one test, `is_file()`, the Windows exe folder for now (the other platforms in slice 6). Win32 `base_data_dir()` (`crates/win32/src/app.rs`) and nox `portable_dir()` (`crates/nox/src/paths.rs`) call it.
- The Win32 window class keyed by folder: `MAIN_CLASS` stays for the default, non-portable folder; every other folder registers `PaTorrentMainWnd.<16 hex of FNV-1a 64 of the lower-cased data dir>`; `instance::forward` (`crates/win32/src/main.rs`) looks for the same name. The NSIS `FindWindow "PaTorrentMainWnd"` check still finds the installed copy, the one it cares about. `REMOTE_CLASS` and the SEC-25 filter unchanged.
- `patorrent_core::util::lockfile` (new): `Lock::take(path) -> io::Result<Lock>` (held for the process's life, a `WouldBlock`-style error when taken). Taken on the data dir in Win32 `app::run` after the mutex, GTK `build()` after GApplication registration, nox `run` after `state_dirs()`. Message strings in the language tables (`FolderInUse`).
- Tests: the class-name hash (pure function), the lock (two handles in one test, one process), nox's `paths` tests updated for the shared test.

### Slice 1: `--profile` everywhere

- `frontend::profile`: `validate(name) -> Result<String, NameError>`; `from_args(args) -> Result<Option<String>, _>` (anywhere among the arguments, never a source); `struct Profile { name: Option<String>, portable: bool, app_root: PathBuf, dir: PathBuf }` resolved once (`OnceLock`), per platform: Win32 `%APPDATA%\PaTorrent` or the portable root; the GTK config and data roots (XDG, macOS `Application Support`); nox's `Dirs` gaining the `profiles/<name>` insertion before `nox/`. `tag()` for titles (`"[work]"`), `app_id()` for GTK (`APP_ID` or `APP_ID.profile_<name with - as _>`).
- Parsers: Win32 `parse_args` and `setup::parse`; GTK `main.rs` early scan (the id is needed before `Application::new`, like `remote::requested()`) and the `command-line` handler skipping the value; nox `parse`, whose `absolutize` / `takes_value` already carry it into service image paths.
- Creation: the GUIs ask (yes/no) when `dir` does not exist, then create it; nox creates silently. A created profile copies nothing from the default (a fresh install).
- Titles: the `NAME_BUILD` call sites (Win32 `app.rs` window title, `App.title`, tray, `sync_title`; GTK `app.rs`, `tray.rs`, `tray_macos.rs`) go through `profile::name_build()`; `{version}` in `TrayTip` / `TitleRemote` carries the tag so no `.lng` table changes.
- Single instance: the Win32 mutex already follows `data_dir()`; GTK `.application_id(profile::app_id())`; `APP_ID` stays for icons, tray, desktop entry, LaunchAgent. Optional Win32 `SetCurrentProcessExplicitAppUserModelID("PaTorrent.<name>")`.
- Remote windows pass `--profile` along (`remote_ui.rs`, `gtk/remote.rs`).
- nox: `--state-dir` / `--config-dir` and systemd's directories still beat the profile; the profile name goes into the `* state:` / `* config:` lines.
- Tests: validation, argument scanning (with `--shell`, sources, `--remote`), root resolution per platform with injected environments (the `nox/src/paths.rs` and `gtk/src/paths.rs` style).

### Slice 2: ports

- A random listen port at profile and portable-root creation (`frontend::profile::first_run_settings`, using `set_listen_port` and `random_port()`); the default profile unchanged.
- `SessionParams.label` -> `UpnpSettings.description` (`core/src/engine/portmap.rs`, `upnp.rs`): `PaTorrent (work)`.
- A listen failure in the status bar: both GUIs catch `Alert::ListenFailed` (today a Logger line only) and show "Port N in use" in the network part; a click opens Preferences > Connection (Win32: the `NM_CLICK` pattern of the update notice).
- Web UI: when the user enables it and `webui.listen` is empty or the default, propose the first free port from 8080 up (`webui_host::Setup`); a bind failure names the port and says another profile may hold it (`Status::Failed`). nox: the same message on bind failure.

### Slice 3: associations, autostart and desktop entries per profile

- Win32 `integration.rs`: `open_command(exe, profile)`, `run_command(exe, profile, minimized)`; `runs(command, exe, profile)` compares the profile too; `upgrade()` only rewrites its own profile's commands; the `Run` value name is `PaTorrent` or `PaTorrent (work)`; `--unregister` removes every association and `Run` value naming this exe. Preferences says next to the box which profile owns the association.
- GTK `integration.rs`: a named profile writes `<APP_ID>.profile_<name>.desktop` (`NoDisplay=true`, `Icon=<APP_ID>`, `Exec=<command> --profile <name> %U`) on first start; the MIME default and the autostart entry point at it; `starts()` compares the profile. Flatpak: `Exec=flatpak run --command=patorrent-gtk <APP_ID> --profile <name> %U`; autostart stays the portal's (default only, the page says so). Verify the dock icon on GNOME and KDE.
- macOS: `agent_path()` per profile (`<APP_ID>.<name>.plist`); file and link opening reach the default profile (Launch Services), documented.

### Slice 4: nox services per profile and the installer

- `service.rs`: `--profile` on `install`, `start`, `stop`, `uninstall` and the new `status`; `NAME` becomes `name(profile)` (`PaTorrentNox`, `PaTorrentNox_work`), display name `PaTorrent nox (work)`; default state `%ProgramData%\PaTorrent\profiles\work\nox`, `prepare()`'s log default accordingly; `status --all` enumerates `PaTorrentNox*` (`EnumServicesStatusExW`) and reports those whose image is in a given folder (an exit code for the installer).
- `contrib/systemd/{system,user}/patorrent-nox@.service` templates (`StateDirectory=patorrent-nox/%i`, `ConfigurationDirectory=...`, `run --profile %i`, no default `--webui`: set per instance in a drop-in); `contrib/freebsd/patorrent_nox` gains rc.subr profiles (`patorrent_nox_profiles`, `_<p>_dir`, `_<p>_save`, `_<p>_args`, one pid file each).
- `patorrent.nsi`: the uninstaller and the upgrade run `patorrent-nox.exe service status --all --in "$INSTDIR"` instead of the ImagePath read; refuse a target folder holding `patorrent.ini`; "remove settings" also removes `profiles\`. Strings in `strings.nsh` for all languages. The Win32 Preferences > Web UI service page passes the GUI's profile to `service install`.
- DAEMON.md updated in the same slice.

### Slice 5: File > Profiles

- Both GUIs: a submenu listing `default` and `profiles/*` (the current one checked and disabled), each item starting a new process with `--profile`; "New Profile..." (a name box with validation, creates and starts); "Create Shortcut..." (Windows: a `.lnk` on the Desktop through `IShellLinkW` + `IPersistFile`, the `Win32_System_Com` and `Win32_UI_Shell` features are already on in `crates/win32/Cargo.toml`; Linux: flips the profile's entry to visible, `Name=PaTorrent (work)`); "Open Profile Folder". Strings in the tables (machine-translated as usual).

### Slice 6: portable mode on Linux/BSD and relative paths

- `profile::portable_root()` per platform: the binary's folder; `$APPIMAGE`'s parent when set; none in the Flatpak (`FLATPAK_ID`). The GTK portable layout puts config and data both in the root (as macOS does today). An unwritable root is an error dialog / nox error, never a fall-back.
- First start in a portable root: `dir.download = ./Downloads`, a random port.
- Relative paths: `SessionParams.portable_root`; the `resume.rs` codec converts `save_path` and `mapped_files`; `frontend::profile::{relativize, absolutize}` used by `GuiOptions`, `TorrentFolders`, `add.rs` recent dirs and awaiting magnets, categories, `rss::store` `save_in`, `ipfilter.path`. Existing absolute paths inside the root become relative on the next save. The CHANGELOG line about the 1.x downgrade.
- `updater::installed()` returns `None` when the portable marker is beside the exe.
- Tests: codec round trips (inside the root, outside it, UNC, `..` rejected), options round trips, a nox integration test in `crates/nox/tests` starting a copied binary with `patorrent.ini` beside it.

### Slice 7 (optional, later): macOS portable mode

- Root = the folder holding `PaTorrent.app` (three levels above `Contents/MacOS/`); an `AppTranslocation` path tells the user to move the app. Needs a Mac (gate 8).

### Slice 8: docs and checks

- `docs/DAEMON.md`, `docs/RELEASING.md` (portable and profile checks), `site/faq.html` (portable on every platform, profiles), `site/download.html`; `tools/windows-clean`: `zip-test.ps1` adds a profile start and a portable relative-path check, `setup-test.ps1` a `PaTorrentNox_x` service and the `profiles\` removal.

## Verification

- Every slice: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, rustfmt; CI on Windows, Linux, macOS, FreeBSD.
- Slice 0: start two portable copies and an installed copy at once on the Windows VM (`tools/windows-clean`), add a torrent to each by a second start, check it lands in the right window; open one folder from two processes and see the message.
- Slices 1-2: `patorrent.exe --profile work` creates after the question, the title shows `[work]`, both profiles run with different ports; the hosted Web UI in two profiles proposes 8080 and 8081; `--profile Work!` is refused.
- Slice 3: double-click a `.torrent` after taking the association in `work`; the Run value per profile on the real registry; GNOME and KDE dock icons for a profile window (the Xubuntu VM, the Flatpak too).
- Slice 4: `service install --profile work` under the real SCM; `systemctl --user enable --now patorrent-nox@work` on the Xubuntu VM; the installer's upgrade and uninstall with a profile service present.
- Slice 6: a portable folder with a torrent downloading into `./Downloads`, moved to another drive letter and to the Linux VM over the Samba share, resumes without a recheck.
- Record what was run in CHANGELOG and the open checks in BACKLOG, as usual.

## Implementation status

Each slice adds its status here (done, changed while building, left out), as LOWPING.md and DISKIO.md do.

- **Slice 0, 1.1.43 (2026-11-03): done.** `frontend::profile::portable_root()` (Windows; `is_file()`) is the one test for Win32's `base_data_dir()` and nox's paths. `app::main_class()` keys the class by folder (`main_class_name`, FNV-1a 64), used by `instance::forward` and the window. `patorrent_core::util::lockfile` (`Lock::take`, `is_taken`, `FILE_NAME`); Win32 takes it in `app::run` for a local window (not demo, not remote) and shows `FolderInUse`; GTK in `build()` for a local window (`lock_library`, in the library folder, the message instead of the engine's start); nox in `run` in the state directory (with `--no-state`, none). The lock file stays after exit (removing it races with a second opener). Changed: nox's error is English, as its other `error:` lines; `tools/windows-clean/desktop/uia.ps1` accepts the keyed class.
- **Slice 1, 1.1.44: done.** `frontend::profile`: `validate`, `split_args` (returns the arguments without the switch, so no parser sees it; stops at `--shell`), `within`, `list`, `Profile { name, portable_root }` behind `init` / `current` (a `OnceLock`, the portable test made once), `tag`, `label`, `args`, `app_id`, `version`, `name_build`. Win32: `app_root()` + `base_data_dir()`, `bad_profile`, `create_profile` (asks before the remote connection too), `SetCurrentProcessExplicitAppUserModelID("PaTorrent.<name>")`. GTK: `paths` take the profile; `say_and_quit` shows a bad name in a window of its own; `create_profile` holds the application while it asks (OK/Cancel, as the other GTK questions). nox: `Opts.profile`, `paths::resolve(.., profile)`. Changed: `{version}` in titles, tooltips and About carries the tag through `profile::version()`, no table changes.
- **Slice 2, 1.1.45: done, with two changes.** `profile::first_run_settings` (a named profile, or a portable root whose ini has no keys) and `name_port`; `SessionParams.label` -> `UpnpSettings.description`; `status::ListenProblem`; Win32 `preferences::show_at` + `PAGE_CONNECTION`, the status bar's `NM_CLICK` on the network part; GTK `preferences::show(.., page)` and a click gesture on the network label; `webui_host::first_free_port`, `start_error`. Changed: (1) the engine moves to the next free port instead of raising `ListenFailed`, so "Port N in use" compares the bound TCP port with the configured one (and still takes a failure); (2) nox has no settings file, so a named nox profile takes a port from its name instead of a random one written at creation.
- **Slice 3, 1.1.46: done, GTK's owner note left out.** Win32 `integration`: `open_command` / `run_command` / `upgrade` / `read` / `write` take the profile, `run_value`, `command_profile`, `Status::other_profile` + `profile_note()` on the General page; `unregister` removes each association as its holder and enumerates `Run` (`RegEnumValueW`). Frontend `integration::entry_shown`, `shown`, profile-aware `starts`. GTK `profile_entry()` at a local start, `command()` with the profile (and `flatpak run` in the Flatpak), `applications_dir()` on the host in the Flatpak; the portal's autostart refused for a named profile with an error; macOS `agent_label()`, `associations_settable()` false for a named profile. Left out: the GTK page does not name the profile that owns the associations (it shows them unchecked); dock icons unverified (gate: Xubuntu VM, KDE, the Flatpak).
