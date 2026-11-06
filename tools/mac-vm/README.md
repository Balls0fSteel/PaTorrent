# Mac VM tools

Scripts for building and looking at the native macOS GUI (`crates/cocoa`, `rookparcel-mac`,
docs/MACOS_GUI.md "Test and VM workflow") on the owner's Intel macOS VM. Everything on the VM
lives under `~/rp/`: `~/rp/<SLICE>/src` (the pushed checkout), `out` (the built app), `run` (its
portable copy), `shots` (pictures and logs), and `~/rp/target` (the cargo target folder all
slices share). Type-check on Windows first; one VM build or run at a time.

| Script | Where | What |
|---|---|---|
| `push.sh SLICE` | Windows (Git Bash) | copies the checkout (tracked and new files, uncommitted changes too; not `docs/`, `site/`, `third_party/`) into `~/rp/SLICE/src`; the VM's scripts get LF line ends |
| `build.sh SLICE [--release]` | VM | under the lock, `cargo build -j 1 -p rookparcel-mac` (at `nice 19`), then `macos_bundle.py native` makes `~/rp/SLICE/out/Rookparcel.app`, checked with `codesign --verify` and `plutil -lint` |
| `run.sh SLICE [SECONDS] [SCRIPT] [-- ARGS]` | VM | under the lock, starts a portable copy with `ROOKPARCEL_SNAPSHOTS=~/rp/SLICE/shots`, runs the AppleScript half way, prints CPU and memory, quits the app |
| `pull.sh SLICE DIR` | Windows (Git Bash) | copies `~/rp/SLICE/shots` into `DIR` |
| `menus.applescript` | VM, through `run.sh` | lists every menu (`menus.txt`) and tries Find, the Edit items in the search field, the View toggles, Full Screen, Minimize and Hide, keeping a snapshot per step |
| `lib.sh` | VM | the paths and the lock, sourced by `build.sh` and `run.sh` |

Typical round:

```bash
tools/mac-vm/push.sh B
ssh -i ~/.ssh/patorrent_vm x@192.168.1.128 'bash ~/rp/B/src/tools/mac-vm/build.sh B'
ssh -i ~/.ssh/patorrent_vm x@192.168.1.128 \
    'bash ~/rp/B/src/tools/mac-vm/run.sh B 60 ~/rp/B/src/tools/mac-vm/menus.applescript -- --demo 100000'
tools/mac-vm/pull.sh B /tmp/shots-B
```

`RP_APPEARANCE=dark` (or `light`) before `run.sh` switches the system appearance for the run and
puts the old one back afterwards. `RP_INI='key=value'` adds lines to the run's `rookparcel.ini`,
which marks the Setup Guide as seen unless `RP_FRESH=1` asks for a first start. `RP_VM` and `RP_KEY` choose another VM or key for `push.sh`
and `pull.sh`.

## The lock

`~/rp/vm.lock` is a directory made with `mkdir`; its `owner` file names the slice, the process
and the time. `build.sh` and `run.sh` wait for it (and say who holds it); one older than 30
minutes is taken over as stale. A script removes only its own lock, also when interrupted.

## What works on the VM (macOS 15.8, VMware, over SSH)

- **Starting the app**: `open -n --env ROOKPARCEL_SNAPSHOTS=... --stdout LOG --stderr LOG
  Rookparcel.app --args ...` starts it in the logged-in GUI session from an SSH shell, as
  Finder would, with the variable and its output in the log. Nothing else was needed
  (`launchctl asuser` was not).
- **The locked screen**: the console session locks after a while
  (`ioreg -n Root -d1 | grep CGSSessionScreenIsLocked` says `Yes`); nobody types its password
  over SSH. While it is locked, `screencapture -x` gives a black picture, no application becomes
  active (so no window is key: Find, the Edit items, Minimize, Zoom and Full Screen stay
  greyed), and typed keys (`keystroke`, `key code`) go nowhere. Ask the owner to unlock the
  VM's screen for those checks.
- **Seeing it**: the windows come from `ROOKPARCEL_SNAPSHOTS` (`window-<n>.png`, drawn by the
  app itself, and `windows.txt` with every visible text); `menus.txt` from System Events holds
  the menus' items, their state and key equivalents. A snapshot's title bar and toolbar can lag
  behind a change of the toolbar (it is drawn in its own layer); the content is current.
- **Driving it**: System Events works over SSH (Accessibility is granted), also while the screen
  is locked, through accessibility actions: opening menus and clicking their items, header
  buttons, selecting table rows, hiding the process. After the toolbar is hidden or shown the
  menu bar can be away from accessibility for a moment, so `menus.applescript` retries. The
  dark mode switch is System Events' appearance preferences.
- **Quitting**: `run.sh` clicks the application menu's Quit item through System Events (which
  goes through `applicationWillTerminate:`) and kills the process if it is still there after
  10 s. Telling the app itself to `quit` would need an Automation permission for it, which
  nobody can grant over SSH.
