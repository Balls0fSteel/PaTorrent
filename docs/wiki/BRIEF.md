# Wiki writing brief (for the agents writing docs/wiki)

`docs/wiki/` is the source of the GitHub wiki (https://github.com/Balls0fSteel/Rookparcel/wiki).
Each `Page-Name.md` here becomes the wiki page `Page-Name`. Images go in `docs/wiki/images/`.
This file is not copied to the wiki.

## Audience and goal

End users of Rookparcel, not developers. Task-oriented: "how do I...". Cover the Windows GUI
(`rookparcel.exe`) first, then say what differs in the GTK GUI (Linux, BSD, macOS) and in the
Web UI (`rookparcel-nox`'s page, or the one a desktop window hosts). Do not describe code.

## Accuracy rules (most important)

- Every claim must be read in the code or the docs. Menu, button, dialog and setting names
  are copied exactly from the English strings in `crates/frontend/src/lang/en.rs`
  (and `help_en.rs` / `docs/HELP_TEXTS.md` for what a setting does). Do not guess a name.
- Where a behaviour is only in the code, read the code (`crates/win32/src`, `crates/gtk/src`,
  `crates/frontend/src`, `crates/nox/src`). If you cannot confirm something, leave it out.
- The wiki describes the current code, version 1.1.63. The latest published release is
  **1.1.40**. When a feature is newer than 1.1.40, say so in one line, e.g.
  "*New in 1.1.59; not in a published release yet.*" Find the version in CHANGELOG.md or
  `docs/archive/` (search by topic), or in `docs/DECISIONS.md`.
- Do not mention internal decision IDs (D42 etc.), gates, agents, or the development plan.
- Leave out unfinished or proposal-only features (e.g. the native macOS GUI D52, the
  Microsoft Store flavour).

## Style

- Model: `docs/SEARCH_ON_WINDOWS.md` (read it). Plain, calm, short sentences, second person.
  No marketing words, no emoji, no exclamation marks.
- Start each page with one or two sentences on what it covers. Then `##` sections, numbered
  steps where the user follows a sequence, tables for option lists.
- UI names in **bold**, menu paths as **Options > Preferences > Connection**, keys as
  <kbd>Ctrl</kbd>+<kbd>N</kbd>, file paths and commands in `code`.
- Links to other wiki pages: `[Adding torrents](Adding-torrents)` (no `.md`, no folder).
  Link to anchors as `[text](Page-Name#section-name)`. Link generously between pages, but only
  to page names in the list below.
- Links to repository files use full URLs: `https://github.com/Balls0fSteel/Rookparcel/blob/main/docs/DAEMON.md`.
- Images: `![Alt text](images/name.png)`, only names from the image list below. Not every
  image must be used; do not invent new image names.
- Length: as long as the task needs; most pages 80-250 lines. No filler.

## Page list and owners

| Page file | Owner | Covers |
|---|---|---|
| Home.md, _Sidebar.md, _Footer.md | lead | index |
| Installing-Rookparcel.md | A | downloads per platform, installer vs zip, SmartScreen/Gatekeeper, AppImage/Flatpak/tar, uninstalling |
| First-start-and-the-Setup-Guide.md | A | the Setup Guide window and what each step sets; skipping; running it again |
| Where-Rookparcel-keeps-its-files.md | A | settings/torrent folders per platform, portable mode, moving to another PC, backing up, reset |
| Profiles.md | A | several independent Rookparcels (profiles), portable profiles, command-line switches |
| Updating-Rookparcel.md | A | the update check, self-update from the installer, beta channel, turning it off, nox |
| Adding-torrents.md | B | .torrent files, magnet links, URLs, drag and drop, browser/file associations, the Add New Torrent window, adding stopped, watched/auto-load folder if any |
| The-main-window.md | B | layout: categories/labels list, torrent list, columns, toolbar, status bar, detail tabs overview, list search box (filter), sorting, tray |
| Managing-torrents.md | B | start/stop/pause/force start, remove (and with data), recheck, move storage, labels, queue order, copy magnet, open folder, multi-select, the right-click menu |
| Torrent-details-and-files.md | B | each detail tab (General, Trackers, Peers, Pieces, Files, Speed, Logger as they exist), file priorities, skipping files, the Files tab filter and menu, Torrent Properties dialog (trackers, limits, ratio) |
| Creating-torrents.md | B | Create New Torrent window, v1/v2/hybrid, trackers, private, piece size, seeding it |
| Importing-from-uTorrent-and-qBittorrent.md | B | the importers |
| Speed-limits-and-scheduler.md | C | global limits, per-torrent limits, the status-bar menus, scheduler grid, anything alternative-speed-like that exists |
| Keep-ping-low.md | C | the keep-ping-low / gamer mode feature: what it does, turning it on, settings |
| Connection-ports-and-firewall.md | C | listening port, UPnP/NAT-PMP, firewall exception, checking the port, uTP/TCP, "Use only this network" (VPN binding), proxies, encryption, IP filter, DHT/PEX/LSD |
| Queueing-and-seeding.md | C | active downloads/seeds limits, seed ratio/time goals, what happens when reached, super-seeding, move completed downloads |
| Disk-and-storage.md | C | smart disk I/O, cache, pre-allocation, move completed, label folders, incomplete-file options that exist |
| Auto-shutdown.md | C | the Auto Shutdown dialog and actions |
| Web-UI.md | D | turning on the Web UI in the Windows/GTK window (Preferences > Web UI) and in nox; signing in; HTTPS; what the page can do; phones |
| Remote-control.md | D | File > Connect to Remote, `--remote`, what works remotely, security |
| Running-rookparcel-nox.md | D | the headless daemon: commands, flags, Windows service, systemd, FreeBSD, files, stopping |
| Sonarr-Radarr-and-Prowlarr.md | D | using nox or the hosted Web UI as a qBittorrent-type download client, categories, paths |
| Searching-for-torrents.md | E | search box filter vs source search, Manage Sources, the search helper on all platforms, Prowlarr/Jackett, adding results; links to the existing page Searching-for-torrents-on-Windows |
| RSS-Downloader.md | E | feeds, filters, episode matching, downloading from feeds, in the Web UI too |
| Preferences-reference.md | F | every Preferences page with a table of its settings and one-line meanings (from HELP_TEXTS.md), plus the Advanced table summary; GTK/Web UI differences |
| Keyboard-shortcuts.md | F | every shortcut in the Windows GUI (and GTK where different), plus F1 help |
| Tray-notifications-and-startup.md | F | tray icon and its menu, close/minimize to tray, notifications, start with the computer, start minimized, file associations |
| Troubleshooting-and-FAQ.md | F | slow or no downloads, port not reachable, firewall, SmartScreen, private trackers (client ID not whitelisted), where logs are, resetting settings, reporting a bug, getting help |

Existing page, do not edit: `Searching-for-torrents-on-Windows` (source `docs/SEARCH_ON_WINDOWS.md`).

## Images (made by agent S from the Windows GUI in demo mode, simulated torrents)

| File | Shows |
|---|---|
| main-window.png | the main window with torrents, a detail tab open |
| add-new-torrent.png | the Add New Torrent window |
| add-url.png | the Add Torrent from URL window |
| torrent-menu.png | the right-click menu of the torrent list |
| tab-general.png | the General detail tab |
| tab-files.png | the Files detail tab |
| tab-peers.png | the Peers detail tab |
| tab-trackers.png | the Trackers detail tab |
| properties.png | the Torrent Properties dialog |
| create-torrent.png | the Create New Torrent window |
| setup-guide.png | the Setup Guide's first page |
| prefs-general.png | Preferences > General |
| prefs-connection.png | Preferences > Connection |
| prefs-bandwidth.png | Preferences > Bandwidth |
| prefs-bittorrent.png | Preferences > BitTorrent |
| prefs-queueing.png | Preferences > Queueing |
| prefs-scheduler.png | Preferences > Scheduler |
| prefs-webui.png | Preferences > Web UI |
| prefs-advanced.png | Preferences > Advanced |
| rss-downloader.png | the RSS Downloader window |
| auto-shutdown.png | the Auto Shutdown window |
| search-sources.png | the Search Sources window |
