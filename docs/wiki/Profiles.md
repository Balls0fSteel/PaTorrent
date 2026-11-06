# Profiles

A profile is a whole Rookparcel of its own: its own settings, torrent list, RSS feeds, ports and window. This page covers making and starting profiles, what each one keeps apart, and the `--profile` switch for shortcuts and the command line.

*New in 1.1.43 to 1.1.50; not in a published release yet.* In 1.1.40 the only way to run two Rookparcels is two portable copies in separate folders ([portable mode](Where-Rookparcel-keeps-its-files#portable-mode)).

## What profiles are for

- A Rookparcel that uses only your VPN, beside a normal one: each profile has its own **Use only this network**, proxy and ports ([Connection, ports and firewall](Connection-ports-and-firewall)).
- Private trackers kept apart from public torrents, each with its own seeding goals and search sources.
- Trying a setting without touching your real setup: `--profile test`.

Several profiles can run at the same time, each in its own window. They share nothing: a torrent in one profile is not seen by another.

## The default profile

Without a profile switch, Rookparcel runs the **default profile**: the folders described in [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files), exactly as before profiles existed. Nothing moves when you start using profiles.

A named profile lives in a `profiles` folder inside the default one:

| | default profile | profile `work` |
|---|---|---|
| Windows | `%APPDATA%\Rookparcel` | `%APPDATA%\Rookparcel\profiles\work` |
| Linux, BSD | `~/.config/rookparcel/gtk`, `~/.local/share/rookparcel/gtk` | `~/.config/rookparcel/profiles/work/gtk`, `~/.local/share/rookparcel/profiles/work/gtk` |
| macOS | `~/Library/Application Support/Rookparcel/mac` | `~/Library/Application Support/Rookparcel/profiles/work/mac` |
| portable folder | the folder | `profiles\work` inside it |

## Profile names

A name has 1 to 32 characters: letters a-z, digits and hyphens (-), starting with a letter. Capital letters are taken as small ones, so `Work` and `work` are the same profile. `default` means the default profile. Any other name is refused with a message that says so.

## Making and starting profiles from the menu

**File > Profiles** lists the default profile (as **Default**) and every named profile. The one this window runs is ticked.

| Item | What it does |
|---|---|
| a profile's name | starts that profile in a window of its own. If it already runs, its window comes to the front |
| **New Profile...** | asks for a name ("Name (letters a-z, digits and hyphens):"), makes the profile and starts it |
| **Create Shortcut...** | Windows: puts a shortcut on your desktop that starts this profile, named after it, for example `Rookparcel (work).lnk`. Linux: puts the profile in your applications menu |
| **Open Profile Folder** | opens this profile's folder |

A new profile starts empty, like a new installation, and opens with the [Setup Guide](First-start-and-the-Setup-Guide).

## Telling the windows apart

A named profile's window shows its name in the title bar and in the tray icon's tooltip: `Rookparcel 1.1.63 [work]`. The default profile shows no name. On Windows each profile has its own taskbar button group, so you can pin them apart.

## What each profile keeps apart

**Ports.** A new profile picks a random listen port between 10000 and 60000 at its first start, so it does not fight the default profile for 6881. The default profile keeps its port. If two profiles are set to the same port anyway, the status bar shows "Port N in use"; click it to open **Options > Preferences > Connection**. See [Connection, ports and firewall](Connection-ports-and-firewall).

Each profile maps its own port on the router (UPnP, NAT-PMP). The mapping is named after the profile, for example `Rookparcel (work)`, so you can tell them apart in the router's list.

**The Web UI.** Each profile's Web UI needs its own port. In the Windows window, when you turn on the Web UI in a profile, **Options > Preferences > Web UI** proposes the first free port from 8080 up. If the port is taken anyway, it says "port N is in use, perhaps by another Rookparcel profile". See [Web UI](Web-UI).

**`.torrent` files and `magnet:` links.** Only one program can open them, so they belong to one profile at a time. Ticking **Associate with .torrent files** or **Associate with magnet: links** in a profile's **Options > Preferences > General** gives them to that profile. On Windows, when another profile has them, the page says so: "The Rookparcel profile "work" opens them now; ticking a box makes this profile open them instead." Double-clicking a `.torrent` file then opens it in the profile that owns the association, starting that profile if it is not running.

The Windows installer's association boxes give them to the default profile.

**Starting with the system.** **Start Rookparcel on system startup** is set per profile, so each profile can start on its own. On Windows they are listed as `Rookparcel` and `Rookparcel (work)`.

**Remote windows.** A window that drives a remote `rookparcel-nox` ([Remote control](Remote-control)) keeps its settings inside the profile it was opened from.

## Starting a profile from the command line

Add `--profile NAME` (or `--profile=NAME`) to the command:

```
rookparcel.exe --profile work
rookparcel-gtk --profile work
```

- It may come anywhere among the arguments, and the name is never taken as a torrent: `rookparcel.exe --profile work file.torrent` adds `file.torrent` to the `work` profile.
- `--profile default`, or no switch, is the default profile.
- When the profile does not exist yet, the window asks first: "The profile "work" does not exist yet. Create it?", and shows the folder it will make. **No** closes without making anything, so a mistyped name does not start an empty Rookparcel.
- It combines with the other switches, for example `--minimized` to start in the tray, or `--remote` to connect to a remote `rookparcel-nox`.

To make a Windows shortcut by hand, add `--profile work` after `rookparcel.exe` in the shortcut's **Target** box. **File > Profiles > Create Shortcut...** does the same for you.

## Profiles and portable mode

A portable Rookparcel has profiles too, in the `profiles` folder inside the portable folder. They travel with it. See [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files#portable-mode).

## GTK differences

- **File > Profiles** works the same. The current profile is shown as the selected item.
- **Create Shortcut...** puts the profile in your applications menu ("Rookparcel (work)"). The profile has a desktop entry of its own, so the dock and the taskbar show its window under its own name and icon.
- In the **Flatpak**, profiles work, but only the default profile can start with the system.

## macOS differences

- **File > Profiles** lists the profiles, **New Profile...** and **Open Profile Folder**; there is no **Create Shortcut...**.
- Opening a `.torrent` file or a `magnet:` link always reaches the default profile: macOS picks the app, not a profile. Starting with the system works per profile.

## Profiles of rookparcel-nox

`rookparcel-nox` takes `--profile NAME` too, to run several daemons on one computer, each with its own torrents, Web UI and service. A daemon profile does not ask before it is made. Its listen port comes from its name, so it stays the same at every start. A GUI profile and a `rookparcel-nox` profile with the same name are separate programs that only share the `profiles/<name>` folder. See [Running rookparcel-nox](Running-rookparcel-nox).

## Removing a profile

1. Close the profile's window (**File > Exit**).
2. If it owns the associations or starts with the system, untick those in its Preferences first, or give them to another profile.
3. Delete its folder, `profiles\<name>`. **File > Profiles > Open Profile Folder** shows where it is.

The downloads of the profile stay where they are.
