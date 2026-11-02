# Clean-machine check of the Windows release files

Two scripts for a clean Windows 10 or 11 machine (the test VM, reached over SSH), run on a draft release's files before publishing it ([RELEASING.md](../../docs/RELEASING.md) step 5). They cover what the release job's own install test cannot: a machine that never had PaTorrent, the default folders, an upgrade, and the all-users mode. First run 2026-10-29 on 1.1.2, all passed.

Copy the draft's `patorrent-<version>-windows-x64.zip` and `-setup.exe` (checked against `SHA256SUMS`), the previous release's setup and the two scripts into one folder on the machine (default `C:\Users\x\patest`), then:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\zip-test.ps1 -Version 1.1.2
powershell -NoProfile -ExecutionPolicy Bypass -File .\setup-test.ps1 -Version 1.1.2 -OldVersion 1.1.0
```

- `zip-test.ps1` unpacks the zip, prints the file versions and signature state, runs `patorrent-nox --version`, starts nox with its Web UI and logs in, starts the GUI in portable mode with its Web UI and logs in (so it really runs, with no desktop), checks `%APPDATA%` stays untouched and asks Defender for detections.
- `setup-test.ps1` installs the previous release for the user with shortcuts and associations, tries the new setup while the GUI runs (it must refuse, exit 2), upgrades, checks the single Settings > Apps entry, the shortcuts, associations and settings that must stay, starts the installed GUI and nox, uninstalls silently as Settings > Apps does, then installs and uninstalls for all users (needs an administrator; an SSH login of one is elevated). It prints `ok` / `FAIL` per check and the number of failures.

Not covered, since an SSH session has no desktop: SmartScreen (files copied over SSH carry no Mark of the Web), the wizard's pages and UAC prompt, "Run PaTorrent" starting it not elevated, drag and drop, and double-clicking a `.torrent` or opening a `magnet:` link. Those stay hand checks ([RELEASING.md](../../docs/RELEASING.md#windows-installer)).

PowerShell variable names ignore case: a parameter must not share a name with a script variable.

## In the desktop session

`desktop/` drives the GUI in the signed-in user's console session from the SSH login, through a one-off scheduled task (`in-session.ps1`); it works with the session locked, so no password is typed. Buttons are pressed by window message (`TDM_CLICK_BUTTON`, `WM_COMMAND`) and windows read through UI Automation; screenshots (`Shot`) only show something when the desktop is unlocked. Each script writes a log in `C:\Users\x\patest`. First run 2026-11-02 on 1.1.32.

```powershell
function P { powershell -NoProfile -ExecutionPolicy Bypass -File @args }
# the GUI's own update (D45): a local "release" 1.1.33 in .\upd (its setup, SHA256SUMS, update.txt with stable=1.1.33)
P .\in-session.ps1 -Script C:\Users\x\patest\serve.ps1 -ScriptArgs '-Delay 100' -Name paserve -High -NoWait
P .\in-session.ps1 -Script C:\Users\x\patest\drive-update.ps1 -ScriptArgs '-Mode user -Cancel -Tag upd-cancel'
P .\in-session.ps1 -Script C:\Users\x\patest\drive-update.ps1 -ScriptArgs '-Mode user -Tag upd-user'
# the closing window (D47): a virtual disk as T:, two torrents (12 and 4 GB), a move to the other drive, Exit, Quit Now
P .\setup-closing.ps1
P .\in-session.ps1 -Script C:\Users\x\patest\drive-closing.ps1 -ScriptArgs '-Tag closing -Wait 0'
P .\setup-closing.ps1 -Down
```

- `drive-update.ps1` starts the installed PaTorrent with `PATORRENT_UPDATE_URL` and `PATORRENT_UPDATE_FILES` pointing at `serve.ps1` (`-Delay` slows it so Cancel can be pressed), takes "Download and install" and answers "Close and install now?" Yes, then checks that a new PaTorrent starts from the same folder, not elevated, with the Settings > Apps entry at the new version. The box comes once per version: remove `update.seen` from `%APPDATA%\PaTorrent\patorrent.ini` before a repeat run. The setup in `upd` can carry the same executables under a higher `/DVERSION`.
- `setup-closing.ps1` also writes the GUI's settings: no Add dialog, the Web UI on `127.0.0.1:8080` with this machine let in without a login (`webui.password` needs a hash in `hash.txt`, e.g. the one `patorrent-nox run --webui-pass` stores in `webui.json`). `drive-closing.ps1` moves the torrents through the Web UI's qBittorrent API, posts File > Exit, reads the closing window and its question by window handle (UI Automation does not list them under the desktop) and checks the folders and a restart; `-NoQuit` waits for the moves instead.
- Not covered: the all-users update (the UAC prompt is on the secure desktop), a sign-out during a move, and anything that needs eyes on the window's layout.
