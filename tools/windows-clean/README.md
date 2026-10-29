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
