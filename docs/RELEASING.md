# Releasing

How a version goes out (PLAN.md D22, Road to 1.0 items 6, 7 and 14). The files are built by
`.github/workflows/release.yml` on GitHub's runners, never on a developer's machine.

## What a release has

| File | What | Built on |
|---|---|---|
| `rookparcel-<version>-windows-x64.zip` | `rookparcel.exe`, `rookparcel-nox.exe` (static CRT, nothing to install, D7), `LICENSE.md`, `THIRD-PARTY-NOTICES.md` | `windows-latest` |
| `rookparcel-<version>-windows-x64-setup.exe` | an installer of the same four files (NSIS, [`packaging/windows`](../packaging/windows/README.md)): for one user or all users, Start menu shortcut, optional desktop shortcut and associations, an uninstaller (see [Windows installer](#windows-installer)) | `windows-latest` |
| `rookparcel-<version>-linux-x86_64.tar.gz`, `-linux-aarch64.tar.gz` | `rookparcel-nox`, `rookparcel-gtk`, the desktop entry, the systemd units (`systemd/system`, `systemd/user`), the same two texts | `ubuntu-24.04`, `ubuntu-24.04-arm` |
| `Rookparcel-<version>-x86_64.AppImage`, `-aarch64.AppImage` | the GTK GUI with GTK 4 and every library it needs but glibc (linuxdeploy and its GTK plugin) | `ubuntu-24.04`, `ubuntu-24.04-arm` |
| `rookparcel-nox-<version>-x86_64.AppImage`, `-aarch64.AppImage` | `rookparcel-nox` with OpenSSL; a terminal program, kept out of the menus | `ubuntu-24.04`, `ubuntu-24.04-arm` |
| `Rookparcel-<version>-x86_64.flatpak`, `-aarch64.flatpak` | the GTK GUI as a single-file Flatpak bundle on the GNOME 51 runtime (see [Flatpak](#flatpak)) | `ubuntu-24.04`, `ubuntu-24.04-arm`, in Flathub's builder container |
| `Rookparcel-<version>-macos.dmg` | `Rookparcel.app`: the native AppKit GUI (`rookparcel-mac`, D61) as one universal binary for Apple silicon and Intel, macOS 11 or later, ad-hoc signed; the two texts | `macos-15` (x86_64 cross-built and joined with `lipo`) |
| `rookparcel-nox-<version>-macos-arm64.tar.gz`, `-macos-x86_64.tar.gz` | `rookparcel-nox` (system libraries only: TLS is Secure Transport, D9), the two texts | `macos-15` (x86_64 cross-built) |
| `SHA256SUMS` | the files' SHA-256 | |
| `ghcr.io/balls0fsteel/rookparcel-nox:<version>`, `:latest` | not a release file: the container image of `rookparcel-nox` ([`Dockerfile`](../Dockerfile), Debian bookworm-slim with OpenSSL 3) for `linux/amd64` and `linux/arm64`, pushed to GitHub's registry; `:latest` only for a version without a suffix (see [Container image](#container-image)) | `ubuntu-24.04`, `ubuntu-24.04-arm` |

The tarballs' binaries use the system's glibc, OpenSSL 3 and GTK 4 (4.12 or later), as Ubuntu
24.04 has them, so they run on distributions about that age or newer. The AppImages carry their
libraries but glibc, so they need glibc 2.39 or newer (Ubuntu 24.04, Fedora 40, Debian 13);
an AppImage started on a desktop without FUSE 2 needs `--appimage-extract-and-run`. The AppImage
icon is provisional (the family `assets/patina-icons/app-icon.txt` names) until the owner picks
one (Road to 1.0 item 10); the Flatpak and the macOS `.app` use the same one.
Earlier releases had `Rookparcel-<version>-arm64.dmg` (`PaTorrent-...` before 1.2.5) instead:
the GTK GUI bundled with Homebrew's GTK 4, Apple silicon only, macOS 15 or later (D23).

### Windows installer

The `windows` job builds the zip, then the installer from the same `target/release` executables
with makensis (`/WX`, so a missing translation or any other warning fails it) and the app icon
`app-icon.txt` names. NSIS is not on `windows-latest` (Windows Server 2025; only the 2022 image
lists it), so the job downloads NSIS 3.13's own installer from SourceForge, checks its SHA-256
against the value winget-pkgs publishes (`NSIS_URL` and `NSIS_SHA256` in the job) and installs
it silently into the job's temporary folder. It then installs the new setup silently for the
runner's user into a temporary folder, with the desktop shortcut and both associations, checks
the files, the shortcuts, the Uninstall key and the associations, runs the installed
`rookparcel-nox.exe --version`, uninstalls silently as Settings > Apps would, and fails if a
file, a shortcut, the key or an association is left. How it works and how to build it by hand:
[packaging/windows/README.md](../packaging/windows/README.md).

What a tester checks on a real Windows 10 or 11 PC (the runner has no desktop session, never
elevates through UAC and starts from a clean registry):

- **SmartScreen.** The setup exe is not signed (see [Not automated](#not-automated)): Windows
  says "Windows protected your PC"; More info > Run anyway. The publisher shows as unknown, and
  the UAC prompt of an installation for all users says "Unknown publisher" too.
- **Install for me** (the default): no UAC prompt; files in `%LOCALAPPDATA%\Programs\Rookparcel`,
  the Start menu entry, Rookparcel under Settings > Apps with its icon, version and publisher;
  "Run Rookparcel" on the last page starts it.
- **Install for anyone using this computer**: the Next button shows the UAC shield, Next asks
  for administrator rights, and the setup goes on elevated (declining the prompt keeps the
  Users page). Files in `C:\Program Files\Rookparcel`, the Start menu entry for every user.
  "Run Rookparcel" must start it *not* elevated: Task Manager > Details > Elevated says No, and
  dragging a `.torrent` file from Explorer onto its window works (an elevated window refuses
  drops from Explorer).
- **Upgrade**: install the previous release, run the new setup: it skips the Users and
  Directory pages, installs over the same folder (no second entry in Settings > Apps), keeps
  the settings, the torrent list and the associations, and the version in Settings > Apps is
  the new one. With Rookparcel running, Setup asks to exit it (Retry / Cancel).
- **Update from Rookparcel** (D45): with the previous release installed and running, its update
  box (10 s after the start, or Help > Check for Updates) offers "Download and install": the
  progress window fills, "Close and install now?" Yes closes Rookparcel, the setup runs silently
  (an all-users installation asks through UAC; "Unknown publisher") and the new version starts
  by itself, not elevated, with the settings and the torrent list kept. Cancel in the progress
  window leaves nothing in `%TEMP%\Rookparcel-update`. The release job checks `/UPDATE=` with a
  stand-in process; this is the real thing.
- **Associations**: with the two boxes ticked, double-clicking a `.torrent` file and opening a
  `magnet:` link in a browser start Rookparcel; Preferences > General shows both ticked. Windows
  11 may still ask which app to use, or keep another program it was told to use (the General
  page's Default apps note).
- **Uninstall** from Settings > Apps: the files, the Start menu and desktop shortcuts, the
  Settings > Apps entry and the associations (and the autostart entry, if set) are gone;
  `%APPDATA%\Rookparcel` stays unless "Settings and torrent list" was ticked; the download folder
  always stays. For an installation for all users the uninstaller asks for administrator rights.
- **nox as a service** from the installation folder (`rookparcel-nox service install`, see
  [DAEMON.md](DAEMON.md)): while it runs, Setup asks to stop it; the uninstaller asks to remove
  the service first.

### macOS

- **One universal app** (D61): the job builds `rookparcel-mac` for `aarch64-apple-darwin` and
  `x86_64-apple-darwin` with `MACOSX_DEPLOYMENT_TARGET=11.0`, and `macos_bundle.py native` joins
  the two with `lipo`. The runner is Apple silicon: it starts the arm64 half; the x86_64 half
  runs there only through Rosetta (`arch -x86_64 ... --version`, a warning when it cannot). Intel
  Macs get nox tarballs too.
- **macOS version:** 11 Big Sur or later for the app; the job writes the highest `minos` of the
  binary's slices into `LSMinimumSystemVersion` and the run's summary. nox needs macOS 11 on
  Apple silicon and 10.14 on Intel (`SecTrustEvaluateWithError`, D9).
- **Ad-hoc signed, not notarised:** Gatekeeper refuses it the first time ("cannot be opened
  because the developer cannot be verified" / "is damaged"). Right-click (or Control-click)
  `Rookparcel.app` > Open, then Open (on macOS 15, System Settings > Privacy & Security > Open
  Anyway); or remove the quarantine flag after copying it to Applications:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Rookparcel.app
  ```

  The nox tarball's binary the same way: `xattr -d com.apple.quarantine rookparcel-nox`.
- **How the `.app` is made** (`.github/scripts/macos_bundle.py native`):
  `Contents/MacOS/Rookparcel` is the universal binary itself, which links only the system's
  libraries and frameworks (the job fails otherwise, and when nox does); `Contents/Info.plist`
  (`NSPrincipalClass` `NSApplication`, `CFBundleLocalizations` for the shipped languages, the
  document and URL types, the local network and Apple events usage texts) and
  `Contents/Resources/Rookparcel.icns`. The job signs the app ad hoc, verifies it, starts it with
  a fresh home folder and uploads a screenshot, its output and its own window pictures
  (`ROOKPARCEL_SNAPSHOTS`, artifact `macos-gui-start`), then makes the dmg with `hdiutil`
  (retried: the runners sometimes say "Resource busy"), verifies it and checks the signature of
  the app inside. `python3 .github/scripts/macos_bundle.py --help` lists its commands;
  `--dry-run FIXTURE.json` runs the logic without a Mac
  (`python3 -I .github/scripts/test_macos_bundle.py`).
- The `.app` declares `.torrent` files and `magnet:` links in its `Info.plist`, so Finder
  offers it for them; both arrive through `application:openURLs:`. Preferences > General sets
  the defaults through LaunchServices (by bundle id, so only from the `.app` and in the default
  profile) and writes "Start Rookparcel on system startup" as
  `~/Library/LaunchAgents/io.github.balls0fsteel.Rookparcel.plist` (`.<profile>` for a named
  one), which starts `Contents/MacOS/Rookparcel` (with `--minimized` for "Start minimized");
  macOS 13 and later show it under System Settings > General > Login Items. Auto Shutdown's
  Sleep and Shut Down are Apple events to loginwindow: macOS may ask once whether Rookparcel may
  control it (`NSAppleEventsUsageDescription`).
- **Library:** `~/Library/Application Support/Rookparcel/mac` (`profiles/<name>/mac`). The first
  start takes over a GTK bundle's `gtk` folder once (renamed to `mac`, paths in its
  `rookparcel.ini` rewritten) when there is no `mac` folder and the old one is not in use.
- **Gate 8** (`.github/workflows/macos-gate8.yml`, run by hand): the same app for the runner's
  own architecture, run portable, from an exFAT stick, from per-profile LaunchAgents and
  translocated.

## The first release as Rookparcel (D59)

Once, before the first tag after the rename:

1. The owner renames the GitHub repository `PaTorrent` to `Rookparcel` (Settings > General). GitHub redirects the repository's old URLs, including release downloads, but not the Pages site: `balls0fsteel.github.io/PaTorrent/` stops answering, so 1.2.x installs hear about no further update (no bridge, by choice). Check that Pages is still enabled and serves `https://balls0fsteel.github.io/Rookparcel/update.txt` after the pages workflow's next run.
2. Push the wiki (`docs/wiki`): four pages have new names (`Installing-Rookparcel`, `Updating-Rookparcel`, `Where-Rookparcel-keeps-its-files`, `Running-rookparcel-nox`). Delete the old pages on the wiki, or leave a one-line pointer in each.
3. The release notes say that PaTorrent users install Rookparcel by hand: the setup removes the installed PaTorrent, and the first start takes over its settings, torrents, associations and autostart entries (docs/DAEMON.md "Coming from PaTorrent" for services).
4. Register `RP` (not `PA`) with libtorrent's `identify_client`, Transmission, WebTorrent and the BEP 20 list (Road to 1.0 item 14).
5. Before submitting to Flathub or the Microsoft Store: both still use the new id and name only; nothing was submitted under the old one.

## Steps

1. Pick the version. Since D37 the version is bumped with every change set (the patch number
   may run high), in `Cargo.toml` (`[workspace.package] version`) and the Windows manifest
   `crates/win32/res/rookparcel.exe.manifest` (`assemblyIdentity version`, four numbers), so a
   release usually only confirms the current one. Add it as the first `<release>` of the
   Flatpak metainfo ([Flatpak](#flatpak)). `PEER_ID_PREFIX` in `crates/core/src/version.rs` is
   a separate counter (`-PA1010-` went out with 1.0.1, `-PA1011-` with 1.1.0, `-PA1013-` with 1.2.0): bump it by one only when the release
   carries an engine change that alters how the client behaves on the wire. Builds that are not
   tagged show `<version>-dev.<commit>` (and `.dirty` from `1_build.bat`) in their title bars,
   About boxes and `--version`; the tagged build shows the plain version.
2. Give it a heading in `CHANGELOG.md` with what is in it since the last release, and write
   `docs/releases/<version>.md`: what it is, the files, how to start each, known limitations.
   The workflow makes that file the release's text.
   - **Website news.** Add an entry for the release at the top of `site/news.html` (format in
     [WEBSITE.md](WEBSITE.md#news)): `id="vX.Y.Z"`, `data-date` the publication date, a one- or
     two-sentence summary and 5-12 tagged bullets from the release notes' "New since" section,
     with links to the GitHub release and `docs/releases/X.Y.Z.md`. Check with
     `python3 site/build.py --releases-json site/testdata/releases.json --strict` and
     `python3 -m unittest discover -s site`. A push to `main` that changes `site/` deploys at
     once, so keep the entry on a branch and push it when the release is published, not before;
     the build then updates `news.atom` and the home page's latest-news line.
3. Check that CI is green on `main` for the commit to be released (Windows, Linux, macOS,
   FreeBSD, MSRV, fuzzing).
4. Tag and push the tag:

   ```bash
   git tag -a v1.0.0 -m "Rookparcel 1.0.0"
   ```

   A pre-release's tag is its version too (`v1.0.0-rc.1`); the workflow marks the release as a
   pre-release when the version has a `-` suffix.

   ```bash
   git push origin v1.0.0
   ```

5. The Release workflow checks that the tag names the workspace version (and stops if not),
   builds the files above and opens a **draft** release with them. Download them, try them (the
   Windows zip and setup on a clean machine with [tools/windows-clean](../tools/windows-clean/README.md), the setup as in [Windows installer](#windows-installer), a
   tarball, the AppImages and the Flatpak on a Linux desktop;
   for macOS, the `macos-gui-start` screenshot or the owner's Intel macOS VM), compare
   `SHA256SUMS`, then publish the draft on GitHub (`gh release edit v1.0.0 --draft=false`).
   To check the GTK or the Mac GUI over SSH, where the screen cannot be captured or read, start
   it with `ROOKPARCEL_SNAPSHOTS=<folder>`: every 2 s it draws its windows into `window-<n>.png`
   there and lists their titles and texts in `windows.txt`, dialogs included (the release job
   uploads the Mac app's in `macos-gui-start`).
6. Publishing starts the Pages workflow, which rebuilds the website's download links and
   `update.txt`, the list the GUIs' update check reads (D32, [WEBSITE.md](WEBSITE.md)). A few
   minutes later, <https://balls0fsteel.github.io/Rookparcel/update.txt> names the new version
   (`stable=` for a release, `prerelease=` for a suffixed one); from then on running copies
   show it in their status bar. The file names above are the website's contract too:
   `site/build.py` sorts a release's files into sections by name, so a new or renamed file
   needs its pattern there.

A dry run without a tag: Actions > Release > Run workflow builds the same files as one
artifact per platform, named `<version>-dev.<commit>`, and makes no release.

### Container image

On a tag the `docker` jobs build the image natively on each architecture's runner, start it
(`--version`) and push it by digest; once the draft release is made, `docker-manifest` joins
the two under `ghcr.io/balls0fsteel/rookparcel-nox:<version>` (and `:latest` without a
suffix). The tags exist before the draft is published, so a dropped draft also needs its image
version deleted (the package's **Package settings** on GitHub) and `:latest` pointed back with
`docker buildx imagetools create -t ghcr.io/balls0fsteel/rookparcel-nox:latest
ghcr.io/balls0fsteel/rookparcel-nox:<previous version>`. A dry run builds both images and
pushes nothing. CI builds and starts the amd64 image when a change touches the `Dockerfile`,
`.dockerignore` or `contrib/docker`.

- **The first push only:** a new GHCR package is private. On GitHub, open the profile's
  **Packages** > `rookparcel-nox` > **Package settings**, set **Change visibility** to public,
  and under **Manage Actions access** check that this repository has write access (the
  workflow creates the package with it). Then `docker pull ghcr.io/balls0fsteel/rookparcel-nox`
  works without a login. Link the package to the repository there too if GitHub did not (the
  image's `org.opencontainers.image.source` label normally does).
- While trying the draft (step 5), pull `:<version>` on a Linux machine (and on arm64 when one
  is at hand) and run the checks of [Docker](wiki/Docker.md): the Web UI answers, the temporary
  password is in `docker logs`, `ROOKPARCEL_WEBUI_PASS` logs in, `docker stop -t 60` ends
  with `* stopped`.

### Profiles and portable mode (D42)

While trying the draft (step 5), beside the scripts' own checks (`zip-test.ps1`: a profile
beside the default one, a portable nox writing `./Downloads`; `setup-test.ps1`: a profile's
service holding the uninstall, a portable target refused):

- **Profiles:** `rookparcel.exe --profile work` asks before making the profile, its title shows
  `[work]`, it runs beside the default profile with another listen port, and File > Profiles
  lists both; `--profile Work!` is refused. On Linux, `rookparcel-gtk --profile work` shows the
  profile's own icon in the dock (GNOME and KDE; the Flatpak too).
- **Associations:** ticking them in `work` takes them over (the General page then names `work`
  in the default profile); a double-clicked `.torrent` opens in `work`.
- **Portable:** an empty `rookparcel.ini` beside the program (the zip, a Linux tarball, beside
  an AppImage) keeps everything in that folder; a torrent downloading into `./Downloads` still
  resumes without a recheck after the folder moves to another drive letter or computer (the
  Samba share between the VMs). A portable folder in Program Files is refused with a message.
- **Services:** `rookparcel-nox service install --profile work` under the real SCM;
  `systemctl --user enable --now rookparcel-nox@work` on the Xubuntu VM.
- **Downgrade:** a portable folder written by 1.1.49 or later needs 1.1.49 or later (older
  versions read `./Downloads` against their working directory); the release notes say so.

## Flatpak

`packaging/flatpak/` holds what the `flatpak` job builds and what a Flathub submission would
use. The job is a matrix like the `linux` job's: x86_64 on `ubuntu-24.04` and aarch64 natively
on `ubuntu-24.04-arm` (no emulation), each in Flathub's builder container
`ghcr.io/flathub-infra/flatpak-github-actions:gnome-51`, which is published for `linux/amd64`
and `linux/arm64` with the GNOME 51 runtime and SDK installed; flatpak-builder installs the
`rust-stable` SDK extension from Flathub, which builds it for both architectures. Each leg runs
every check below (the metainfo, the icon, the `cargo-sources.json` comparison, the linter) and
uploads its bundle as the `flatpak-x86_64` or `flatpak-aarch64` artifact; its flatpak-builder
cache key carries the architecture.

- `io.github.balls0fsteel.Rookparcel.yml`: the flatpak-builder manifest (GNOME 51 runtime, the
  Rust SDK extension, an offline `cargo build --release --locked -p rookparcel-gtk`). It
  installs the binary, the desktop entry the GUI itself writes (`contrib/linux/`, kept current
  by a test in `crates/frontend/src/integration.rs`), the metainfo, the icons at 16 to 512
  pixels (the provisional icon, `ICON_DIR` in the manifest, which the `flatpak` job checks
  against `assets/patina-icons/app-icon.txt`) and `LICENSE.md` with the notices.
- `io.github.balls0fsteel.Rookparcel.metainfo.xml`: the AppStream metadata. Its first
  `<release>` must be the workspace version (the job stops otherwise): add a release there in
  step 2 of [Steps](#steps). The job validates it with `appstreamcli validate`. A pre-release is
  written with a tilde (`1.0.0~rc.1` for `1.0.0-rc.1`): AppStream sorts `1.0.0-rc.1` after
  `1.0.0`, and the validator fails on releases out of order (as the first `v1.0.0` run did).
- `cargo-sources.json`: every crate in `Cargo.lock` as a flatpak-builder source, so the build
  needs no network. Regenerate it whenever `Cargo.lock` changes, with the generator from
  [flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools) (the job pins its
  commit in `CARGO_GENERATOR`; it needs `aiohttp`, `PyYAML` and `tomlkit`):

  ```bash
  python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
  ```

  The job regenerates it before building and compares (byte for byte: the pinned generator
  reproduces the committed file exactly), so a stale file only gives a warning there and the
  build uses the regenerated one; Flathub builds the committed one. In Flathub's builder image
  pip and the generator both run with `python -I`: the image sets `PYTHONPATH` to a
  site-packages that has `aiohttp`, so a plain `pip install` in the venv found it "already
  satisfied" and installed nothing, and the generator, isolated, then had no `aiohttp`.
- `lint-exceptions.json`: the two linter errors a Flathub submission asks exceptions for (see
  [Flathub submission](#flathub-submission)), in the linter's local format, so the job's
  advisory `flatpak-builder-lint --exceptions --user-exceptions` run prints only what Flathub
  would still refuse.

**Installing the bundle.** The bundle carries the application, not the runtime, which comes
from Flathub:

```bash
flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user Rookparcel-1.0.0-x86_64.flatpak   # or -aarch64.flatpak on ARM
flatpak run io.github.balls0fsteel.Rookparcel
```

**Building it by hand** (on Linux, with `flatpak-builder` and Flathub added):

```bash
flatpak-builder --user --install-deps-from=flathub --force-clean --repo=repo build-dir packaging/flatpak/io.github.balls0fsteel.Rookparcel.yml
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo repo Rookparcel.flatpak io.github.balls0fsteel.Rookparcel
```

**Sandbox permissions** (`finish-args` in the manifest):

| Permission | Why |
|---|---|
| `--share=network` | BitTorrent, trackers, DHT, the remote mode |
| `--share=ipc`, `--socket=wayland`, `--socket=fallback-x11`, `--device=dri` | a GTK 4 window on Wayland or X11, with GPU rendering |
| `--filesystem=home`, `--filesystem=xdg-download` | downloads go to any folder the user picks, and `.torrent` files are opened from anywhere, as in µTorrent (below) |
| `--talk-name=org.kde.StatusNotifierWatcher` | the tray icon (`crates/gtk/src/tray.rs`). In a Flatpak the item registers its unique bus name (`:1.42`) with the watcher, as Qt, KDE Frameworks and Electron do, instead of owning `org.kde.StatusNotifierItem-<pid>-1`, so it needs no `--own-name` (Flathub's linter refuses `org.kde.*`) |
| `--talk-name=org.freedesktop.Notifications` | notifications from a remote-mode window, which is not the bus's application and so cannot use `GNotification` |
| `--system-talk-name=org.freedesktop.login1` | Auto Shutdown's Standby, Hibernate and Shut Down (`crates/gtk/src/shutdown.rs`) |

The portals need no permission: a sandbox can always reach `org.freedesktop.portal.*` on the
session bus, so autostart through the Background portal (below) adds nothing to `finish-args`.

The home folder is the tradeoff. The portals would let the sandbox see only what the user picks
in a file chooser, but a torrent client keeps writing to its folders long after the dialog, for
every torrent, across restarts, and moves completed downloads elsewhere; the document portal
grants files, not lasting folder access the engine can use, so Rookparcel would have to be
redesigned around it. `--filesystem=home` gives the program what µTorrent users expect, at the
cost of the sandbox not protecting the home folder from it. Folders outside home (another disk
under `/mnt` or `/media`) need `flatpak override --user --filesystem=/mnt/data
io.github.balls0fsteel.Rookparcel`.

**Inside a Flatpak, Preferences > General** leaves the associations to the Flatpak
(PLAN.md Known TODOs, "GTK desktop integration"): their boxes are greyed under a note, nothing
about them is read or written, and "Check association on startup" never asks. The exported
entry's `MimeType` already offers Rookparcel for both types in the desktop's own "Open with" and
default-application settings; GIO would write the default handlers to the sandbox's own
`~/.var/app/<id>/config/mimeapps.list`, which the host does not read.

"Start Rookparcel on system startup" and "Start minimized" work there through the Background
portal (`crates/gtk/src/portal.rs`): `org.freedesktop.portal.Background.RequestBackground` with
`autostart`, `commandline` `["rookparcel-gtk"]` or `["rookparcel-gtk", "--minimized"]` (the
manifest's `command`, as the sandbox sees it; a test keeps them equal), `dbus-activatable`
false and the box's text as `reason`. The portal writes the host's
`~/.config/autostart/io.github.balls0fsteel.Rookparcel.desktop` (`flatpak run --command=...`),
and turning the box off sends the same request with `autostart` false. The desktop may ask the
user first (GNOME does, once); the answer comes on the request's `Response` signal and cannot be
read back later, so it is kept in `background.ini` in the settings folder
(`~/.var/app/io.github.balls0fsteel.Rookparcel/config/rookparcel/gtk/`) and the boxes show it.
Apply does not wait for the answer; a refusal or failure shows the desktop-integration error.

**Flathub submission.** Flathub builds from the public source (D25), so a submission is a pull
request to [flathub/flathub](https://github.com/flathub/flathub) (the `new-pr` branch) that
becomes the `flathub/io.github.balls0fsteel.Rookparcel` repository, holding:

- this manifest, with the `dir` source replaced by the release's `git` source (`url` the
  repository, `tag` and `commit`) and `cargo-sources.json` beside it;
- the metainfo, which must pass Flathub's `flatpak-builder-lint` (screenshots taken of the GTK
  GUI, at URLs on the repository, are needed there and are not in the file yet);
- exceptions for the two linter errors the manifest keeps, `finish-args-home-filesystem-access`
  and `finish-args-login1-system-talk-name` (below; the manifest has no `--own-name`, which
  Flathub's linter would refuse, see the table).

**Linter exceptions.** Flathub records exceptions in the linter's own repository, not in the
app's: a pull request to
[flathub/flatpak-builder-lint](https://github.com/flathub/flatpak-builder-lint)'s
`flatpak_builder_lint/staticfiles/exceptions.json` adds one entry for the app, keyed by the
repository (`stable`, or `beta` for the beta remote), each error with its reason:

```json
"io.github.balls0fsteel.Rookparcel": {
    "stable": {
        "finish-args-home-filesystem-access": "<reason>",
        "finish-args-login1-system-talk-name": "<reason>"
    }
}
```

`flathub.json` in the app's repository has no place for them (it holds build options such as
`only-arches`). Both errors are "granted on sufficient explanation" in Flathub's linter
documentation. **Flathub asks that exception requests are not written with an LLM, and says
that exceptions which open the sandbox, `home` among them, are not granted when there are signs
of LLM use in the software or in the request.** So the reasons and the pull request must be the
owner's own words, and the owner should weigh that policy against how this project is made
before asking for `home` at all (the alternative is below). The facts a reason can draw on:

- *Home.* The engine writes each torrent's files into whatever folder the user chose for it
  (Preferences > Directories, the Add dialog's Save In, a label's folder), keeps writing there
  for days across restarts, moves finished downloads to another chosen folder ("Move completed
  downloads to", Set Download Location), and watches a folder for new `.torrent` files and moves
  them to others (`.torrent` folders). Resume data names those folders, so they must still be
  reachable after a restart without asking again. The narrower alternative exists: the file
  chooser portal with `directory` set grants lasting access to a picked folder through the
  document portal, surviving restarts. It would mean every one of those folders is picked
  through the portal (including folders typed or remembered from before), the engine's paths
  become `/run/user/<uid>/doc/<id>/<name>`, which the GUI would have to show as the real
  folder, and every piece read and write goes through the document portal's FUSE file system,
  which is slow for a torrent's many small random writes. That is a redesign of the
  directories handling, not done; `--filesystem=xdg-download` alone would work for users who
  keep everything in Downloads.
- *logind.* Options > Auto Shutdown can, when downloads finish, put the machine to sleep
  (Standby), hibernate it or shut it down, as µTorrent does; the GUI calls logind's `Suspend`,
  `Hibernate` and `PowerOff` with `interactive` true (polkit still decides) after checking
  `CanSuspend` and the others (`crates/gtk/src/shutdown.rs`), and nothing else on that bus.
  There is no portal for it: the Inhibit portal only holds sleep off, and the desktops' session
  managers (`org.gnome.SessionManager`, KDE's ksmserver) are desktop-specific, show a
  confirmation that defeats an unattended shutdown, and do not hibernate. Without the
  permission Auto Shutdown can only quit Rookparcel.

The license is the open question: Flathub accepts software that is not free, and
`PolyForm-Noncommercial-1.0.0` is an SPDX identifier, so the metainfo names it and Flathub shows
the app as proprietary. Flathub redistributing the build is distribution, which the license
allows for noncommercial purposes; the owner should confirm that Flathub's hosting fits that
before submitting. The `io.github.balls0fsteel` id needs the GitHub account
`Balls0fSteel` to be verified on Flathub.

## Not automated

- Code signing (Windows Authenticode, the macOS notarisation): none; the Windows executables,
  the setup and its uninstaller are unsigned, and the `.app` is ad-hoc signed (D23), which
  proves nothing about who made it.
- The `PA` peer-id registrations and the parser security review before 1.0 (Road to 1.0
  item 14).
