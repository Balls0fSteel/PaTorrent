# Rookparcel wiki

Rookparcel is a lean BitTorrent client in the spirit of µTorrent 2.2.1: small, fast and native. This wiki explains how to use it: installing it, adding and managing torrents, the settings, the Web UI and the headless daemon.

Rookparcel comes as three programs:

| Program | Where | What it is |
|---|---|---|
| `rookparcel.exe` | Windows 10 and 11 | the Windows window (GUI) |
| `rookparcel-gtk` | Linux, BSD, macOS | the GTK 4 window (GUI): the same main window and dialogs, with a few differences noted on each page |
| `rookparcel-nox` | all of them | runs without a window, controlled from a browser through its Web UI |

Most pages describe the Windows window and then say what differs in the GTK window and the Web UI.

**Version:** the wiki follows the current code (1.1.63). The newest published release is [1.1.40](https://github.com/Balls0fSteel/Rookparcel/releases/latest); features added after it are marked on their pages.

## Getting started

- [Installing Rookparcel](Installing-Rookparcel)
- [First start and the Setup Guide](First-start-and-the-Setup-Guide)
- [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files) (and portable mode)
- [Profiles](Profiles): several independent Rookparcels side by side
- [Updating Rookparcel](Updating-Rookparcel)

## Everyday use

- [Adding torrents](Adding-torrents): files, magnet links, URLs, the Add New Torrent window
- [The main window](The-main-window)
- [Managing torrents](Managing-torrents): start, stop, remove, labels, queue order
- [Torrent details and files](Torrent-details-and-files): the detail tabs, choosing files, Torrent Properties
- [Creating torrents](Creating-torrents)
- [Importing from µTorrent and qBittorrent](Importing-from-uTorrent-and-qBittorrent)
- [Searching for torrents](Searching-for-torrents) and the step-by-step [Searching for torrents on Windows](Searching-for-torrents-on-Windows)
- [RSS Downloader](RSS-Downloader)

## Speed, network and disk

- [Speed limits and the scheduler](Speed-limits-and-scheduler)
- [Keep ping low](Keep-ping-low) (gamer mode)
- [Connection, ports and firewall](Connection-ports-and-firewall), VPNs and proxies
- [Queueing and seeding](Queueing-and-seeding)
- [Disk and storage](Disk-and-storage)
- [Auto Shutdown](Auto-shutdown)

## Remote and headless

- [Web UI](Web-UI)
- [Remote control](Remote-control): a desktop window driving a Rookparcel elsewhere
- [Running rookparcel-nox](Running-rookparcel-nox): the headless daemon and services
- [Sonarr, Radarr and Prowlarr](Sonarr-Radarr-and-Prowlarr)

## Reference

- [Preferences reference](Preferences-reference)
- [Keyboard shortcuts](Keyboard-shortcuts)
- [Tray, notifications and startup](Tray-notifications-and-startup)
- [Troubleshooting and FAQ](Troubleshooting-and-FAQ)

## More

- [Website](https://balls0fsteel.github.io/Rookparcel/): downloads, news, FAQ, benchmarks
- [Releases](https://github.com/Balls0fSteel/Rookparcel/releases)
- [Bug reports and questions](https://github.com/Balls0fSteel/Rookparcel/issues)
- Developer documentation: the repository's [docs](https://github.com/Balls0fSteel/Rookparcel/tree/main/docs) folder
