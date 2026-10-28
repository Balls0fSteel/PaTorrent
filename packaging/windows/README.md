# Windows installer

`patorrent.nsi` builds `patorrent-<version>-windows-x64-setup.exe`, the installer that each
release has beside the portable zip (which stays as it is: settings beside the exe when a
`patorrent.ini` is there, D7). It is an NSIS 3 Unicode installer with the Modern UI 2;
`strings.nsh` holds its own few texts in the 34 languages PaTorrent has. The release workflow
builds and tests it ([docs/RELEASING.md](../../docs/RELEASING.md#windows-installer)).

## Building it

NSIS 3 (3.13 here and in CI; `makensis` on `PATH`) and the release executables:

```powershell
cargo build --release --locked -p patorrent -p patorrent-nox
$family = (Get-Content assets\patina-icons\app-icon.txt -Raw).Trim()
makensis /NOCD /WX /DVERSION=1.0.0-rc.1 /DBINDIR=target\release "/DICON=assets\patina-icons\exports\app\$family.ico" /DOUTFILE=patorrent-1.0.0-rc.1-windows-x64-setup.exe packaging\windows\patorrent.nsi
```

Run it from the repository's root. `/NOCD` keeps makensis there (it otherwise moves into the
script's folder), so the paths above, `LICENSE.md` and `THIRD-PARTY-NOTICES.md` are found;
`/WX` makes any warning an error, so a language missing one of `strings.nsh`'s texts fails the
build. The four definitions are required:

| | |
|---|---|
| `VERSION` | the version as the release names it (`1.0.0-rc.1`); the setup's file version is its first three numbers and a 0 (`1.0.0.0`), as in the Windows manifest |
| `BINDIR` | the folder with `patorrent.exe` and `patorrent-nox.exe` |
| `ICON` | the installer's and uninstaller's icon: the `.ico` of the family `assets/patina-icons/app-icon.txt` names |
| `OUTFILE` | where the setup is written |

## What it does

**Pages:** language (the system's language preselected), welcome, license (`LICENSE.md`, the
PolyForm Noncommercial License 1.0.0), users, components, folder, progress, finish ("Run
PaTorrent").

**For me or for everyone.** "Install just for me" (the default) needs no administrator rights
and installs into `%LOCALAPPDATA%\Programs\PaTorrent`, as µTorrent installed per user; the
Start menu entry is the user's and the Settings > Apps entry is under `HKEY_CURRENT_USER`.
"Install for anyone using this computer" installs into `C:\Program Files\PaTorrent`, with the
Start menu entry for all users and the Settings > Apps entry under `HKEY_LOCAL_MACHINE`. The
setup runs without elevation (`RequestExecutionLevel user`); choosing all users starts a second
copy of it through the UAC prompt (ShellExecuteEx with "runas"), which carries on from the
components page while the first waits for it and ends with its exit code. Declining the prompt
returns to the users page. NSIS's own `MultiUser.nsh` cannot do that: it decides the rights at
start, so an administrator would get a UAC prompt for a per-user installation too, and the
plug-ins that elevate halfway (UAC, NsisMultiUser) are third-party binaries.

**Components:** PaTorrent (the two programs, the texts, the Start menu entry, the uninstaller),
a desktop shortcut, "Associate with .torrent files" and "Associate with magnet: links", the
last three ticked on a new installation. The associations are written by
`patorrent.exe --associate torrent,magnet`, the same code as Preferences > General
(`crates/win32/src/integration.rs`), under `HKEY_CURRENT_USER` of the user who runs the setup
(after an elevation with another administrator's password, that administrator's). Other users
of an installation for everyone associate from Preferences > General, or answer PaTorrent's
"Check association on startup" question.

**Running PaTorrent at the end** never runs it elevated: after an installation for everyone
the setup is elevated, so it asks Explorer to start PaTorrent, which starts it as the signed-in
user. A silent installation starts nothing.

**Upgrades.** The setup looks for an installation in the Settings > Apps entry (the user's
first, then all users'), takes its mode and folder, skips the users and folder pages and
installs over it. On an upgrade the desktop shortcut box shows whether there is one (unticking
it removes it) and the association boxes start unticked: the associations stay as they are.

**Files in use.** Nothing is killed. When PaTorrent's window is open, or `patorrent.exe` in the
folder is busy (another user's session), the setup and the uninstaller ask to exit it, with
Retry and Cancel. When `patorrent-nox.exe` in the folder is busy (the PaTorrent nox service
installed from there by hand, see
[docs/DAEMON.md](../../docs/DAEMON.md), or nox in a console), they ask
to stop it (`sc stop PaTorrentNox` from an administrator prompt); start the service again after
the upgrade. The uninstaller also asks to remove the service first
(`patorrent-nox service uninstall`) while it is registered to the folder's
`patorrent-nox.exe`, which it would otherwise leave starting a deleted file.

**Settings > Apps entry** (`Software\Microsoft\Windows\CurrentVersion\Uninstall\PaTorrent`):
DisplayName PaTorrent, DisplayVersion, Publisher DukeCorp1996, DisplayIcon, InstallLocation,
EstimatedSize, URLInfoAbout (the website), HelpLink and URLUpdateInfo (the releases page),
NoModify, NoRepair, UninstallString, QuietUninstallString (`/S`), and InstallerLanguage (the
language the uninstaller then speaks).

**The uninstaller** first runs `patorrent.exe --unregister`, which removes the associations,
the Default apps entry (`RegisteredApplications`) and the "Start PaTorrent on system startup"
entry where they name this installation's `patorrent.exe` (another copy's, a portable one say,
stay), then removes the files, the shortcuts and its Settings > Apps entry. The folder goes
only when that leaves it empty. "Settings and torrent list" (unticked) also removes what
PaTorrent keeps in `%APPDATA%\PaTorrent`, by name: `patorrent.ini`, `state` (the torrent list
and resume data), `rss.dat`, `remote.ini` and `remote` (remote connections), `ipfilter.dat`
and `nox` (nox's state when it ran from a console); the folder goes only when that leaves it
empty. Downloaded files are never touched.

## Command line

| | |
|---|---|
| `/S` | silent (NSIS): no pages, nothing started at the end; a question (PaTorrent running) is answered Cancel, and the exit code is then 2 |
| `/D=<folder>` | the folder (NSIS); must be the last argument, without quotes |
| `/CURRENTUSER`, `/ALLUSERS` | the mode, without the users page (default: the installation found, else just me). `/ALLUSERS` without administrator rights asks through UAC, also when silent |
| `/DESKTOPSHORTCUT` | the desktop shortcut (a silent installation makes none otherwise) |
| `/ASSOCIATE=torrent,magnet` | the associations (`torrent`, `magnet` or both); a silent installation or an upgrade leaves them as they are otherwise |

The uninstaller takes `/S`. `/ELEVATED`, `/CONTINUE` and `/LANG=<id>` are what a setup passes
to its elevated copy.

Examples: `patorrent-1.0.0-windows-x64-setup.exe /S` (for the current user, or an upgrade of
the installation found), `patorrent-1.0.0-windows-x64-setup.exe /S /ALLUSERS /ASSOCIATE=torrent,magnet`
from an administrator prompt, `"%LOCALAPPDATA%\Programs\PaTorrent\uninstall.exe" /S`.

## Limits

- x64 only (and Windows 11 on Arm, which runs x64 programs); it refuses 32-bit Windows and
  anything older than Windows 10.
- Not signed: SmartScreen warns, and the UAC prompt says "Unknown publisher".
- The associations, autostart and `%APPDATA%\PaTorrent` belong to one user: an uninstaller of
  an installation for everyone cleans them up for the user running it only.
