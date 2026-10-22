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
| `SHA256SUMS` | the files' SHA-256 | |

The tarballs' binaries use the system's glibc, OpenSSL 3 and GTK 4 (4.12 or later), as Ubuntu
24.04 has them, so they run on distributions about that age or newer. The AppImages carry their
libraries but glibc, so they need glibc 2.39 or newer (Ubuntu 24.04, Fedora 40, Debian 13);
an AppImage started on a desktop without FUSE 2 needs `--appimage-extract-and-run`. The AppImage
icon is provisional (the recommended candidate, `APP_ICON` in the workflow) until the owner picks
one (Road to 1.0 item 10). Flatpak and the macOS `.app` come later (Road to 1.0 items 7 and 8).

## Steps

1. Pick the version and set it in `Cargo.toml` (`[workspace.package] version`) and in
   `PEER_ID_PREFIX` in `crates/core/src/version.rs` (`-PA0100-` is 0.1.0; a test fails when the
   two differ).
2. Give it a heading in `CHANGELOG.md` with what is in it since the last release.
3. Check that CI is green on `main` for the commit to be released (Windows, Linux, FreeBSD,
   MSRV, fuzzing).
4. Tag and push the tag:

   ```bash
   git tag -a v1.0.0 -m "PaTorrent 1.0.0"
   ```

   ```bash
   git push origin v1.0.0
   ```

5. The Release workflow checks that the tag names the workspace version (and stops if not),
   builds the files above and opens a **draft** release with them. Download them, try them (the
   Windows zip on a clean machine, a tarball and the AppImages on a Linux desktop), compare
   `SHA256SUMS`, then publish the draft on GitHub.

A dry run without a tag: Actions > Release > Run workflow builds the same files as one
artifact per platform, named `<version>-dev.<commit>`, and makes no release.

## Not automated

- Code signing (Windows Authenticode, the macOS notarisation): none; the `.app` will be ad-hoc
  signed (D23).
- The `PA` peer-id registrations and the parser security review before 1.0 (Road to 1.0
  item 14).
