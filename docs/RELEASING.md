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
| `PaTorrent-<version>-arm64.dmg` | `PaTorrent.app`: the GTK GUI with GTK 4 and every library it needs from Homebrew, ad-hoc signed; the two texts | `macos-15` (Apple silicon) |
| `patorrent-nox-<version>-macos-arm64.tar.gz`, `-macos-x86_64.tar.gz` | `patorrent-nox` (system libraries only: TLS is Secure Transport, D9), the two texts | `macos-15` (x86_64 cross-built) |
| `SHA256SUMS` | the files' SHA-256 | |

The tarballs' binaries use the system's glibc, OpenSSL 3 and GTK 4 (4.12 or later), as Ubuntu
24.04 has them, so they run on distributions about that age or newer. The AppImages carry their
libraries but glibc, so they need glibc 2.39 or newer (Ubuntu 24.04, Fedora 40, Debian 13);
an AppImage started on a desktop without FUSE 2 needs `--appimage-extract-and-run`. The AppImage
icon is provisional (the recommended candidate, `APP_ICON` in the workflow) until the owner picks
one (Road to 1.0 item 10). Flatpak comes later (Road to 1.0 item 7).

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
   Windows zip on a clean machine, a tarball and the AppImages on a Linux desktop; for macOS,
   the `macos-gui-start` screenshot, as nobody here has a Mac), compare
   `SHA256SUMS`, then publish the draft on GitHub.

A dry run without a tag: Actions > Release > Run workflow builds the same files as one
artifact per platform, named `<version>-dev.<commit>`, and makes no release.

## Not automated

- Code signing (Windows Authenticode, the macOS notarisation): none; the `.app` is ad-hoc
  signed (D23), which proves nothing about who made it.
- The `PA` peer-id registrations and the parser security review before 1.0 (Road to 1.0
  item 14).
