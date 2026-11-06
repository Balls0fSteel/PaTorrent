# Installing Rookparcel

This page lists the downloads for each platform, how to install and start each one the first time, and how to remove Rookparcel again. After the first start, see [First start and the Setup Guide](First-start-and-the-Setup-Guide).

## Where to download

Every release is on the [Releases page](https://github.com/Balls0fSteel/Rookparcel/releases). The latest published release is 1.1.40. Each release has notes with the first-run steps per platform, and a `SHA256SUMS` file with the checksum of every download.

| Platform | File | What is inside |
|---|---|---|
| Windows 10 / 11, x64, installer | `rookparcel-<version>-windows-x64-setup.exe` | `rookparcel.exe` (the window) and `rookparcel-nox.exe` (the headless daemon), installed with a Start menu entry and an uninstaller |
| Windows 10 / 11, x64, portable | `rookparcel-<version>-windows-x64.zip` | the same two programs; nothing to install |
| Linux desktop, x86_64 or aarch64 | `Rookparcel-<version>-<arch>.AppImage` | the GTK 4 window with its libraries bundled (needs glibc 2.39 or newer: Ubuntu 24.04, Fedora 40, Debian 13 or later) |
| Linux desktop, Flatpak | `Rookparcel-<version>-<arch>.flatpak` | the GTK 4 window on the GNOME 51 runtime from Flathub |
| Linux, plain binaries | `rookparcel-<version>-linux-<arch>.tar.gz` | `rookparcel-gtk`, `rookparcel-nox`, the desktop entry and the systemd units; they use the system's GTK 4 (4.12 or newer) and OpenSSL 3 |
| Linux server, headless | `rookparcel-nox-<version>-<arch>.AppImage` | `rookparcel-nox` with OpenSSL bundled |
| macOS, Apple silicon or Intel | `Rookparcel-<version>-macos.dmg` | `Rookparcel.app`, the native macOS window, one app for both kinds of Mac (macOS 11 Big Sur or newer) |
| macOS, headless | `rookparcel-nox-<version>-macos-<arch>.tar.gz` | `rookparcel-nox` for Apple silicon (`arm64`, macOS 11 or newer) and Intel (`x86_64`, macOS 10.14 or newer) |

The window on Linux and the BSDs is the GTK version; on macOS it is a native Mac app. Both work like the Windows version; where they differ, the wiki pages say so. For running the headless daemon, see [Running rookparcel-nox](Running-rookparcel-nox).

*The native macOS app is not in a published release yet. Earlier releases have `Rookparcel-<version>-arm64.dmg` instead: the GTK window bundled for Apple silicon, macOS 15 or newer.*

## Windows

### Installer or zip

- **The installer** puts Rookparcel in your Start menu, can associate it with `.torrent` files and `magnet:` links, adds an entry under Settings > Apps for uninstalling, and lets Rookparcel update itself (see [Updating Rookparcel](Updating-Rookparcel)).
- **The zip** needs no installation: unzip it anywhere and start `rookparcel.exe`. It can run in portable mode, with its settings and torrents kept in its own folder (see [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files#portable-mode)). It does not update itself; a newer version is a new zip.

Both are the same programs. Rookparcel needs 64-bit Windows 10 or later. Windows 11 on Arm runs the x64 build.

### Running the installer

1. Run `rookparcel-<version>-windows-x64-setup.exe`.
2. Windows SmartScreen may show "Windows protected your PC", because the setup is not code-signed. Click **More info**, then **Run anyway**.
3. Pick the setup's language, read the welcome page and accept the licence (the PolyForm Noncommercial License 1.0.0).
4. **Choose Users:**
   - **Install just for me** (the default) needs no administrator rights. Rookparcel goes into `%LOCALAPPDATA%\Programs\Rookparcel`.
   - **Install for anyone using this computer** installs into `C:\Program Files\Rookparcel`. Windows asks for administrator rights, and the prompt says "Unknown publisher". If you decline it, you are back on this page.
5. **Components:** Rookparcel itself, **Desktop shortcut**, **Associate with .torrent files** and **Associate with magnet: links**. The last three are ticked on a new installation.
6. Pick the folder, or keep the one shown, and let it install.
7. On the last page, leave "Run Rookparcel" ticked to start it.

The associations are written for the user who runs the setup. Other users of an installation for everyone can turn them on in **Options > Preferences > General**.

The setup refuses a folder that holds a `rookparcel.ini` file: that folder is a portable Rookparcel. Choose another folder.

### Installing over an older version

Run the newer setup. It finds the installed Rookparcel, skips the users and folder pages and installs over it in the same folder. Your settings and torrents stay where they are. On an upgrade the association boxes start unticked, which leaves the associations as they are.

If Rookparcel is running, the setup asks you to exit it (**File > Exit**) and click **Retry**. If `rookparcel-nox.exe` from the same folder is running, for example as the Rookparcel nox service, it asks you to stop it first. From an administrator prompt:

```
sc stop RookparcelNox
```

Start the service again after the upgrade. An installed copy can also do all of this by itself: see [Updating Rookparcel](Updating-Rookparcel).

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
rookparcel-1.1.40-windows-x64-setup.exe /S /ALLUSERS /ASSOCIATE=torrent,magnet
```

### The zip

1. Unzip `rookparcel-<version>-windows-x64.zip` into any folder you can write to.
2. Start `rookparcel.exe`. SmartScreen warns the first time, as above: **More info**, then **Run anyway**.

Settings and torrents go to `%APPDATA%\Rookparcel`, the same place an installed copy uses. To keep them in the zip's folder instead, put an empty file named `rookparcel.ini` beside `rookparcel.exe` before the first start. See [Portable mode](Where-Rookparcel-keeps-its-files#portable-mode).

To open `.torrent` files and `magnet:` links with the zip copy, tick the associations in **Options > Preferences > General**. They then name this copy's `rookparcel.exe`, so they stop working if you move or delete its folder.

### Checking a download

Each release's `SHA256SUMS` lists the SHA-256 of every file. In PowerShell:

```
Get-FileHash .\rookparcel-1.1.40-windows-x64-setup.exe -Algorithm SHA256
```

The hash should match the file's line in `SHA256SUMS`.

## Linux

### AppImage

1. Download `Rookparcel-<version>-x86_64.AppImage` (or `-aarch64` on ARM).
2. Make it executable and start it:

   ```bash
   chmod +x Rookparcel-1.1.40-x86_64.AppImage
   ./Rookparcel-1.1.40-x86_64.AppImage
   ```

3. Without FUSE 2 on the system, start it with `--appimage-extract-and-run` added.

The AppImage needs glibc 2.39 or newer (Ubuntu 24.04, Fedora 40, Debian 13 or later). Settings go to `~/.config/rookparcel/gtk`, torrents to `~/.local/share/rookparcel/gtk`. A `rookparcel.ini` file beside the `.AppImage` makes it portable instead (*new in 1.1.49; not in a published release yet*).

### Flatpak

The bundle holds the application; the runtime comes from Flathub. Add Flathub once, then install the bundle:

```bash
flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user Rookparcel-1.1.40-x86_64.flatpak
flatpak run io.github.balls0fsteel.Rookparcel
```

After that it is in your applications menu. The Flatpak has no update check of its own and no portable mode. Its `.torrent` and `magnet:` associations are the desktop's choice: pick Rookparcel as the default program in your desktop's settings.

### Plain binaries (tar.gz)

`rookparcel-<version>-linux-<arch>.tar.gz` holds `rookparcel-gtk`, `rookparcel-nox`, the desktop entry and the systemd units (`systemd/system`, `systemd/user`). The binaries use the system's GTK 4 (4.12 or newer) and OpenSSL 3, as Ubuntu 24.04 has them, so they run on distributions about that age or newer.

Unpack it and start `rookparcel-gtk`. A `rookparcel.ini` file beside the binary makes it portable (*new in 1.1.49; not in a published release yet*). For the systemd units, see [Running rookparcel-nox](Running-rookparcel-nox).

### Headless server

`rookparcel-nox-<version>-<arch>.AppImage` is the daemon alone, with OpenSSL bundled; it stays out of the menus. See [Running rookparcel-nox](Running-rookparcel-nox).

## macOS

1. Open `Rookparcel-<version>-macos.dmg` and drag `Rookparcel.app` to Applications.
2. The app is ad-hoc signed and not notarised, so Gatekeeper refuses it the first time. Right-click (or Control-click) the app, choose **Open**, then click **Open**. If macOS offers no Open button, open **System Settings > Privacy & Security** and click **Open Anyway** there.
3. Or remove the quarantine flag in Terminal:

   ```bash
   xattr -dr com.apple.quarantine /Applications/Rookparcel.app
   ```

The app runs on macOS 11 Big Sur or newer, on Apple silicon and Intel Macs alike: the one `Rookparcel.app` holds both. Settings and torrents go to `~/Library/Application Support/Rookparcel/mac`.

### Upgrading from the GTK version

Earlier releases had a GTK version of `Rookparcel.app` for Apple silicon. Replace it with the new `Rookparcel.app` in Applications. At its first start, the new app renames the old app's `gtk` folder in `~/Library/Application Support/Rookparcel` to `mac`, and each profile's too, so your settings, torrents and profiles carry over. This happens once, and only while no `mac` folder is there yet. Quit the old app first: while it runs, the new one says "Another Rookparcel is already using this folder" and closes. See [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files#macos).

A "Start Rookparcel on system startup" entry the old app made keeps working. A portable folder is used as it is.

For the headless daemon, unpack `rookparcel-nox-<version>-macos-<arch>.tar.gz` and run `rookparcel-nox` from Terminal ([Running rookparcel-nox](Running-rookparcel-nox)).

## Uninstalling

### Windows installer

1. Exit Rookparcel (**File > Exit**).
2. Open **Settings > Apps**, find **Rookparcel** and choose **Uninstall**.
3. The uninstaller has one option, **Settings and torrent list**, unticked by default. Tick it to also remove what Rookparcel keeps in `%APPDATA%\Rookparcel`: the settings, the torrent list and its progress, RSS feeds, remote connections, the default IP filter, nox's files and the profiles.

The uninstaller removes the associations, the Default apps entry and the "Start Rookparcel on system startup" entry that name this installation's `rookparcel.exe`. Those of another copy, a portable one for example, stay. It never touches your downloaded files.

If the Rookparcel nox service runs from the installation folder, the uninstaller asks you to remove it first. From an administrator prompt:

```
"%LOCALAPPDATA%\Programs\Rookparcel\rookparcel-nox.exe" service uninstall --all
```

(use your own installation folder). A silent uninstall: `"%LOCALAPPDATA%\Programs\Rookparcel\uninstall.exe" /S`.

The settings belong to each Windows user. Uninstalling an installation for everyone cleans them up for the user who runs the uninstaller only.

### Windows zip

1. If you ticked associations or "Start Rookparcel on system startup", untick them in **Options > Preferences > General** first.
2. Exit Rookparcel and delete its folder.
3. Delete `%APPDATA%\Rookparcel` too, unless the copy was portable (then everything was in its folder).

### Linux and macOS

- **AppImage or tar.gz:** delete the file or folder. To remove the settings and the torrent list, delete `~/.config/rookparcel` and `~/.local/share/rookparcel`.
- **Flatpak:** `flatpak uninstall --user io.github.balls0fsteel.Rookparcel`.
- **macOS:** drag `Rookparcel.app` to the Trash. The settings and torrent list are in `~/Library/Application Support/Rookparcel`.

Downloaded files stay where they are in every case. For the full list of folders, see [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files).
