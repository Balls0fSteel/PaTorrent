# Releasing

How a version goes out (PLAN.md D22, Road to 1.0 items 6, 7 and 14). The files are built by
`.github/workflows/release.yml` on GitHub's runners, never on a developer's machine.

## What a release has

| File | What | Built on |
|---|---|---|
| `patorrent-<version>-windows-x64.zip` | `patorrent.exe`, `patorrent-nox.exe` (static CRT, nothing to install, D7), `LICENSE.md`, `THIRD-PARTY-NOTICES.md` | `windows-latest` |
| `patorrent-<version>-linux-x86_64.tar.gz`, `-linux-aarch64.tar.gz` | `patorrent-nox`, `patorrent-gtk`, the desktop entry, the systemd units (`systemd/system`, `systemd/user`), the same two texts | `ubuntu-24.04`, `ubuntu-24.04-arm` |
| `PaTorrent-<version>-x86_64.AppImage`, `-aarch64.AppImage` | the GTK GUI with GTK 4 and every library it needs but glibc (linuxdeploy and its GTK plugin) | `ubuntu-24.04`, `ubuntu-24.04-arm` |
| `patorrent-nox-<version>-x86_64.AppImage`, `-aarch64.AppImage` | `patorrent-nox` with OpenSSL; a terminal program, kept out of the menus | `ubuntu-24.04`, `ubuntu-24.04-arm` |
| `PaTorrent-<version>-x86_64.flatpak` | the GTK GUI as a single-file Flatpak bundle on the GNOME 51 runtime (see [Flatpak](#flatpak)) | `ubuntu-24.04`, in Flathub's builder container |
| `PaTorrent-<version>-arm64.dmg` | `PaTorrent.app`: the GTK GUI with GTK 4 and every library it needs from Homebrew, ad-hoc signed; the two texts | `macos-15` (Apple silicon) |
| `patorrent-nox-<version>-macos-arm64.tar.gz`, `-macos-x86_64.tar.gz` | `patorrent-nox` (system libraries only: TLS is Secure Transport, D9), the two texts | `macos-15` (x86_64 cross-built) |
| `SHA256SUMS` | the files' SHA-256 | |

The tarballs' binaries use the system's glibc, OpenSSL 3 and GTK 4 (4.12 or later), as Ubuntu
24.04 has them, so they run on distributions about that age or newer. The AppImages carry their
libraries but glibc, so they need glibc 2.39 or newer (Ubuntu 24.04, Fedora 40, Debian 13);
an AppImage started on a desktop without FUSE 2 needs `--appimage-extract-and-run`. The AppImage
icon is provisional (the recommended candidate, `APP_ICON` in the workflow) until the owner picks
one (Road to 1.0 item 10); the Flatpak and the macOS `.app` use the same one.

### macOS

- **The GUI is Apple silicon (arm64) only.** The runner's Homebrew has no Intel GTK to bundle;
  an Intel `.app` needs an x86_64 Homebrew (or a universal GTK build) first. Intel Macs get
  nox.
- **macOS version:** the `.app` needs what the Homebrew bottles it bundles were built for, which
  is the runner's macOS (15, Sequoia, for bottles built on `macos-15`); the job writes the exact
  version (the highest `minos` of every Mach-O inside) into `LSMinimumSystemVersion` and the
  run's summary. nox needs macOS 11 on Apple silicon and 10.14 on Intel
  (`SecTrustEvaluateWithError`, D9).
- **Ad-hoc signed, not notarised:** Gatekeeper refuses it the first time ("cannot be opened
  because the developer cannot be verified" / "is damaged"). Right-click (or Control-click)
  `PaTorrent.app` > Open, then Open; or remove the quarantine flag after copying it to
  Applications:

  ```bash
  xattr -dr com.apple.quarantine /Applications/PaTorrent.app
  ```

  The nox tarball's binary the same way: `xattr -d com.apple.quarantine patorrent-nox`.
- **How the `.app` is made** (`.github/scripts/macos_bundle.py`): `Contents/MacOS/PaTorrent`
  is a small launcher (`.github/scripts/macos_launcher.c`) that points GLib and GTK at the
  bundle's schemas, icon themes, GdkPixbuf loaders and GIO modules, then becomes
  `Contents/MacOS/patorrent-gtk`. Every non-system library is in `Contents/Frameworks`, found by
  walking `otool -L` and rewritten to `@executable_path/../Frameworks`; the job fails when any
  Mach-O inside (or nox) still names a library outside the bundle. It then starts the GUI with
  Homebrew moved away and uploads a screenshot and its output (artifact `macos-gui-start`).
  `python3 .github/scripts/macos_bundle.py --help` lists its commands; `--dry-run FIXTURE.json`
  runs the logic without a Mac.
- The `.app` declares `.torrent` files and `magnet:` links in its `Info.plist`, so Finder
  offers it for them; the GUI does not yet take the open requests macOS sends (Apple events,
  not command-line arguments), PLAN.md D23.

## Steps

1. Pick the version and set it in `Cargo.toml` (`[workspace.package] version`) and in
   `PEER_ID_PREFIX` in `crates/core/src/version.rs` (`-PA0100-` is 0.1.0; a test fails when the
   two differ).
2. Give it a heading in `CHANGELOG.md` with what is in it since the last release.
3. Check that CI is green on `main` for the commit to be released (Windows, Linux, macOS,
   FreeBSD, MSRV, fuzzing).
4. Tag and push the tag:

   ```bash
   git tag -a v1.0.0 -m "PaTorrent 1.0.0"
   ```

   ```bash
   git push origin v1.0.0
   ```

5. The Release workflow checks that the tag names the workspace version (and stops if not),
   builds the files above and opens a **draft** release with them. Download them, try them (the
   Windows zip on a clean machine, a tarball, the AppImages and the Flatpak on a Linux desktop;
   for macOS, the `macos-gui-start` screenshot, as nobody here has a Mac), compare
   `SHA256SUMS`, then publish the draft on GitHub.

A dry run without a tag: Actions > Release > Run workflow builds the same files as one
artifact per platform, named `<version>-dev.<commit>`, and makes no release.

## Flatpak

`packaging/flatpak/` holds what the `flatpak` job builds and what a Flathub submission would
use:

- `io.github.balls0fsteel.PaTorrent.yml`: the flatpak-builder manifest (GNOME 51 runtime, the
  Rust SDK extension, an offline `cargo build --release --locked -p patorrent-gtk`). It
  installs the binary, the desktop entry the GUI itself writes (`contrib/linux/`, kept current
  by a test in `crates/frontend/src/integration.rs`), the metainfo, the icons at 16 to 512
  pixels (the provisional icon, `ICON_DIR` in the manifest) and `LICENSE.md` with the notices.
- `io.github.balls0fsteel.PaTorrent.metainfo.xml`: the AppStream metadata. Its first
  `<release>` must be the workspace version (the job stops otherwise): add a release there in
  step 2 of [Steps](#steps). The job validates it with `appstreamcli validate`.
- `cargo-sources.json`: every crate in `Cargo.lock` as a flatpak-builder source, so the build
  needs no network. Regenerate it whenever `Cargo.lock` changes, with the generator from
  [flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools) (the job pins its
  commit in `CARGO_GENERATOR`; it needs `aiohttp`, `PyYAML` and `tomlkit`):

  ```bash
  python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
  ```

  The job regenerates it before building, so a stale file only gives a warning there; Flathub
  builds the committed one.

**Installing the bundle.** The bundle carries the application, not the runtime, which comes
from Flathub:

```bash
flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user PaTorrent-1.0.0-x86_64.flatpak
flatpak run io.github.balls0fsteel.PaTorrent
```

**Building it by hand** (on Linux, with `flatpak-builder` and Flathub added):

```bash
flatpak-builder --user --install-deps-from=flathub --force-clean --repo=repo build-dir packaging/flatpak/io.github.balls0fsteel.PaTorrent.yml
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo repo PaTorrent.flatpak io.github.balls0fsteel.PaTorrent
```

**Sandbox permissions** (`finish-args` in the manifest):

| Permission | Why |
|---|---|
| `--share=network` | BitTorrent, trackers, DHT, the remote mode |
| `--share=ipc`, `--socket=wayland`, `--socket=fallback-x11`, `--device=dri` | a GTK 4 window on Wayland or X11, with GPU rendering |
| `--filesystem=home`, `--filesystem=xdg-download` | downloads go to any folder the user picks, and `.torrent` files are opened from anywhere, as in µTorrent (below) |
| `--talk-name=org.kde.StatusNotifierWatcher` | the tray icon (`crates/gtk/src/tray.rs`) |
| `--own-name=org.kde.*` | **temporary**: the tray owns `org.kde.StatusNotifierItem-<pid>-1`, and a flatpak name wildcard covers only whole name elements. To go once the tray falls back to its unique bus name in a sandbox (Flathub refuses this permission) |
| `--talk-name=org.freedesktop.Notifications` | notifications from a remote-mode window, which is not the bus's application and so cannot use `GNotification` |
| `--system-talk-name=org.freedesktop.login1` | Auto Shutdown's Standby, Hibernate and Shut Down (`crates/gtk/src/shutdown.rs`) |

The home folder is the tradeoff. The portals would let the sandbox see only what the user picks
in a file chooser, but a torrent client keeps writing to its folders long after the dialog, for
every torrent, across restarts, and moves completed downloads elsewhere; the document portal
grants files, not lasting folder access the engine can use, so PaTorrent would have to be
redesigned around it. `--filesystem=home` gives the program what µTorrent users expect, at the
cost of the sandbox not protecting the home folder from it. Folders outside home (another disk
under `/mnt` or `/media`) need `flatpak override --user --filesystem=/mnt/data
io.github.balls0fsteel.PaTorrent`.

**Inside a Flatpak, Preferences > General** behaves differently (PLAN.md Known TODOs, "GTK
desktop integration"): the application's entry is the one the Flatpak exports, so the
associations need no file of their own, but GIO writes the default handlers to the sandbox's
own `~/.var/app/<id>/config/mimeapps.list`, which the desktop most likely does not read (not
checked yet), and "Start PaTorrent on system startup" needs the Background portal, which is not
done yet. The exported entry's `MimeType` already offers PaTorrent for both types in the
desktop's own "Open with" settings.

**Flathub submission.** Flathub builds from the public source (D25), so a submission is a pull
request to [flathub/flathub](https://github.com/flathub/flathub) (the `new-pr` branch) that
becomes the `flathub/io.github.balls0fsteel.PaTorrent` repository, holding:

- this manifest, with the `dir` source replaced by the release's `git` source (`url` the
  repository, `tag` and `commit`) and `cargo-sources.json` beside it;
- the metainfo, which must pass Flathub's `flatpak-builder-lint` (screenshots taken of the GTK
  GUI, at URLs on the repository, are needed there and are not in the file yet);
- no `--own-name=org.kde.*` (see the table) and a reviewer's agreement to `--filesystem=home`.

The license is the open question: Flathub accepts software that is not free, and
`PolyForm-Noncommercial-1.0.0` is an SPDX identifier, so the metainfo names it and Flathub shows
the app as proprietary. Flathub redistributing the build is distribution, which the license
allows for noncommercial purposes; the owner should confirm that Flathub's hosting fits that
before submitting. The `io.github.balls0fsteel` id needs the GitHub account
`Balls0fSteel` to be verified on Flathub.

## Not automated

- Code signing (Windows Authenticode, the macOS notarisation): none; the `.app` is ad-hoc
  signed (D23), which proves nothing about who made it.
- The `PA` peer-id registrations and the parser security review before 1.0 (Road to 1.0
  item 14).
