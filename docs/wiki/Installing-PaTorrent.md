# Installing PaTorrent

This page lists the downloads for each platform, how to install and start each one the first time, and how to remove PaTorrent again. After the first start, see [First start and the Setup Guide](First-start-and-the-Setup-Guide).

## Where to download

Every release is on the [Releases page](https://github.com/Balls0fSteel/PaTorrent/releases). The latest published release is 1.1.40. Each release has notes with the first-run steps per platform, and a `SHA256SUMS` file with the checksum of every download.

| Platform | File | What is inside |
|---|---|---|
| Windows 10 / 11, x64, installer | `patorrent-<version>-windows-x64-setup.exe` | `patorrent.exe` (the window) and `patorrent-nox.exe` (the headless daemon), installed with a Start menu entry and an uninstaller |
| Windows 10 / 11, x64, portable | `patorrent-<version>-windows-x64.zip` | the same two programs; nothing to install |
| Linux desktop, x86_64 or aarch64 | `PaTorrent-<version>-<arch>.AppImage` | the GTK 4 window with its libraries bundled (needs glibc 2.39 or newer: Ubuntu 24.04, Fedora 40, Debian 13 or later) |
| Linux desktop, Flatpak | `PaTorrent-<version>-<arch>.flatpak` | the GTK 4 window on the GNOME 51 runtime from Flathub |
| Linux, plain binaries | `patorrent-<version>-linux-<arch>.tar.gz` | `patorrent-gtk`, `patorrent-nox`, the desktop entry and the systemd units; they use the system's GTK 4 (4.12 or newer) and OpenSSL 3 |
| Linux server, headless | `patorrent-nox-<version>-<arch>.AppImage` | `patorrent-nox` with OpenSSL bundled |
| macOS, Apple silicon | `PaTorrent-<version>-arm64.dmg` | `PaTorrent.app`, the GTK 4 window with its libraries bundled (macOS 15 or newer) |
| macOS, headless | `patorrent-nox-<version>-macos-<arch>.tar.gz` | `patorrent-nox` for Apple silicon (`arm64`, macOS 11 or newer) and Intel (`x86_64`, macOS 10.14 or newer) |

The window on Linux, the BSDs and macOS is the GTK version. It works like the Windows version; where it differs, the wiki pages say so. Intel Macs get `patorrent-nox` only. For running the headless daemon, see [Running patorrent-nox](Running-patorrent-nox).

## Windows

### Installer or zip

- **The installer** puts PaTorrent in your Start menu, can associate it with `.torrent` files and `magnet:` links, adds an entry under Settings > Apps for uninstalling, and lets PaTorrent update itself (see [Updating PaTorrent](Updating-PaTorrent)).
- **The zip** needs no installation: unzip it anywhere and start `patorrent.exe`. It can run in portable mode, with its settings and torrents kept in its own folder (see [Where PaTorrent keeps its files](Where-PaTorrent-keeps-its-files#portable-mode)). It does not update itself; a newer version is a new zip.

Both are the same programs. PaTorrent needs 64-bit Windows 10 or later. Windows 11 on Arm runs the x64 build.

### Running the installer

1. Run `patorrent-<version>-windows-x64-setup.exe`.
2. Windows SmartScreen may show "Windows protected your PC", because the setup is not code-signed. Click **More info**, then **Run anyway**.
3. Pick the setup's language, read the welcome page and accept the licence (the PolyForm Noncommercial License 1.0.0).
4. **Choose Users:**
   - **Install just for me** (the default) needs no administrator rights. PaTorrent goes into `%LOCALAPPDATA%\Programs\PaTorrent`.
   - **Install for anyone using this computer** installs into `C:\Program Files\PaTorrent`. Windows asks for administrator rights, and the prompt says "Unknown publisher". If you decline it, you are back on this page.
5. **Components:** PaTorrent itself, **Desktop shortcut**, **Associate with .torrent files** and **Associate with magnet: links**. The last three are ticked on a new installation.
6. Pick the folder, or keep the one shown, and let it install.
7. On the last page, leave "Run PaTorrent" ticked to start it.

The associations are written for the user who runs the setup. Other users of an installation for everyone can turn them on in **Options > Preferences > General**.

The setup refuses a folder that holds a `patorrent.ini` file: that folder is a portable PaTorrent. Choose another folder.

### Installing over an older version

Run the newer setup. It finds the installed PaTorrent, skips the users and folder pages and installs over it in the same folder. Your settings and torrents stay where they are. On an upgrade the association boxes start unticked, which leaves the associations as they are.

If PaTorrent is running, the setup asks you to exit it (**File > Exit**) and click **Retry**. If `patorrent-nox.exe` from the same folder is running, for example as the PaTorrent nox service, it asks you to stop it first. From an administrator prompt:

```
sc stop PaTorrentNox
```

Start the service again after the upgrade. An installed copy can also do all of this by itself: see [Updating PaTorrent](Updating-PaTorrent).

### Silent installation

The setup takes these switches, for scripts and administrators:

| Switch | Effect |
|---|---|
| `/S` | silent: no pages, nothing started at the end |
| `/D=<folder>` | the installation folder; must be the last argument, without quotes |
| `/CURRENTUSER`, `/ALLUSERS` | for me or for everyone, without the users page. `/ALLUSERS` without administrator rights asks through the Windows prompt, also when silent |
| `/DESKTOPSHORTCUT` | make the desktop shortcut (a silent installation makes none otherwise) |
| `/ASSOCIATE=torrent,magnet` | the associations: `torrent`, `magnet` or both |

For example, from an administrator prompt:

```
patorrent-1.1.40-windows-x64-setup.exe /S /ALLUSERS /ASSOCIATE=torrent,magnet
```

### The zip

1. Unzip `patorrent-<version>-windows-x64.zip` into any folder you can write to.
2. Start `patorrent.exe`. SmartScreen warns the first time, as above: **More info**, then **Run anyway**.

Settings and torrents go to `%APPDATA%\PaTorrent`, the same place an installed copy uses. To keep them in the zip's folder instead, put an empty file named `patorrent.ini` beside `patorrent.exe` before the first start. See [Portable mode](Where-PaTorrent-keeps-its-files#portable-mode).

To open `.torrent` files and `magnet:` links with the zip copy, tick the associations in **Options > Preferences > General**. They then name this copy's `patorrent.exe`, so they stop working if you move or delete its folder.

### Checking a download

Each release's `SHA256SUMS` lists the SHA-256 of every file. In PowerShell:

```
Get-FileHash .\patorrent-1.1.40-windows-x64-setup.exe -Algorithm SHA256
```

The hash should match the file's line in `SHA256SUMS`.

## Linux

### AppImage

1. Download `PaTorrent-<version>-x86_64.AppImage` (or `-aarch64` on ARM).
2. Make it executable and start it:

   ```bash
   chmod +x PaTorrent-1.1.40-x86_64.AppImage
   ./PaTorrent-1.1.40-x86_64.AppImage
   ```

3. Without FUSE 2 on the system, start it with `--appimage-extract-and-run` added.

The AppImage needs glibc 2.39 or newer (Ubuntu 24.04, Fedora 40, Debian 13 or later). Settings go to `~/.config/patorrent/gtk`, torrents to `~/.local/share/patorrent/gtk`. A `patorrent.ini` file beside the `.AppImage` makes it portable instead (*new in 1.1.49; not in a published release yet*).

### Flatpak

The bundle holds the application; the runtime comes from Flathub. Add Flathub once, then install the bundle:

```bash
flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user PaTorrent-1.1.40-x86_64.flatpak
flatpak run io.github.balls0fsteel.PaTorrent
```

After that it is in your applications menu. The Flatpak has no update check of its own and no portable mode. Its `.torrent` and `magnet:` associations are the desktop's choice: pick PaTorrent as the default program in your desktop's settings.

### Plain binaries (tar.gz)

`patorrent-<version>-linux-<arch>.tar.gz` holds `patorrent-gtk`, `patorrent-nox`, the desktop entry and the systemd units (`systemd/system`, `systemd/user`). The binaries use the system's GTK 4 (4.12 or newer) and OpenSSL 3, as Ubuntu 24.04 has them, so they run on distributions about that age or newer.

Unpack it and start `patorrent-gtk`. A `patorrent.ini` file beside the binary makes it portable (*new in 1.1.49; not in a published release yet*). For the systemd units, see [Running patorrent-nox](Running-patorrent-nox).

### Headless server

`patorrent-nox-<version>-<arch>.AppImage` is the daemon alone, with OpenSSL bundled; it stays out of the menus. See [Running patorrent-nox](Running-patorrent-nox).

## macOS

1. Open `PaTorrent-<version>-arm64.dmg` and drag `PaTorrent.app` to Applications.
2. The app is ad-hoc signed and not notarised, so Gatekeeper refuses it the first time. Right-click (or Control-click) the app, choose **Open**, then click **Open**. If macOS offers no Open button, open **System Settings > Privacy & Security** and click **Open Anyway** there.
3. Or remove the quarantine flag in Terminal:

   ```bash
   xattr -dr com.apple.quarantine /Applications/PaTorrent.app
   ```

The window needs macOS 15 or newer on Apple silicon. Settings and torrents go to `~/Library/Application Support/PaTorrent/gtk`.

For the headless daemon, unpack `patorrent-nox-<version>-macos-<arch>.tar.gz` and run `patorrent-nox` from Terminal ([Running patorrent-nox](Running-patorrent-nox)).

## Uninstalling

### Windows installer

1. Exit PaTorrent (**File > Exit**).
2. Open **Settings > Apps**, find **PaTorrent** and choose **Uninstall**.
3. The uninstaller has one option, **Settings and torrent list**, unticked by default. Tick it to also remove what PaTorrent keeps in `%APPDATA%\PaTorrent`: the settings, the torrent list and its progress, RSS feeds, remote connections, the default IP filter, nox's files and the profiles.

The uninstaller removes the associations, the Default apps entry and the "Start PaTorrent on system startup" entry that name this installation's `patorrent.exe`. Those of another copy, a portable one for example, stay. It never touches your downloaded files.

If the PaTorrent nox service runs from the installation folder, the uninstaller asks you to remove it first. From an administrator prompt:

```
"%LOCALAPPDATA%\Programs\PaTorrent\patorrent-nox.exe" service uninstall --all
```

(use your own installation folder). A silent uninstall: `"%LOCALAPPDATA%\Programs\PaTorrent\uninstall.exe" /S`.

The settings belong to each Windows user. Uninstalling an installation for everyone cleans them up for the user who runs the uninstaller only.

### Windows zip

1. If you ticked associations or "Start PaTorrent on system startup", untick them in **Options > Preferences > General** first.
2. Exit PaTorrent and delete its folder.
3. Delete `%APPDATA%\PaTorrent` too, unless the copy was portable (then everything was in its folder).

### Linux and macOS

- **AppImage or tar.gz:** delete the file or folder. To remove the settings and the torrent list, delete `~/.config/patorrent` and `~/.local/share/patorrent`.
- **Flatpak:** `flatpak uninstall --user io.github.balls0fsteel.PaTorrent`.
- **macOS:** drag `PaTorrent.app` to the Trash. The settings and torrent list are in `~/Library/Application Support/PaTorrent`.

Downloaded files stay where they are in every case. For the full list of folders, see [Where PaTorrent keeps its files](Where-PaTorrent-keeps-its-files).
