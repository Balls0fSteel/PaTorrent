# Rookparcel

## Short hand-written note by author

Thank you for your interest in the project! You can see our project goals below and you can find benchmarks on the Github pages website. Please, help the project out by testing, making issue reports on our tracker, and getting it whitelisted on trackers.

## Will the client get me banned on a private tracker?

Before the very first public release the client has been thoroughly tested. It passes all interop tests (transfer to/from other clients) and regular tests. I've been using it since the first binary and had no issues whatsoever. But, for safety, we do use our own peerID.

## Is it safe to use?

Security is utmost importance for us. It has been designed with security in mind, but we also run proactive scans on the codebase as well. Still, if you have attack Claude access for example, feel free to comb through our code (please) and contact me on the security email address. I will be sure to fix any issues ASAP.

## Was it written by LLMs?

They heavily helped out, yes. Several LLM models were used to ensure speed and security. Porting an entire libtorrent engine and several other functionality is no simple task.

## How can I help?

- Test, test, test! If you encounter any issues, please open a ticket and include all the details, reproduction tests.
- If you are a company who rents out Apple machines (such as Mac Mini), please contact me. I'd love to have a remote Mac for testing.
- If you are a seedbox company - just contact me. Our license do not allow just stealing the code or hosting Rookparcel, but I'm more than willing to allow this. It's our goal to make it the best client out there. Just contact me, we will work something out. Even a tiny percentage, or a donation if there is enough users - anything works. Just to keep the project alive and help pay us the bills.

## Donation

Please donate - if you can. Your support means the world to us, but only give if you are comfortably able to do so. Your own financial well-being always comes first. Tokens are expensive. I have other projects too (published and work in progress), your support will be put to good use I assure you.

> [!NOTE]
> **Rookparcel 1.1.40 is the latest stable release.** It has been used mostly by its author so far, and nothing is code-signed. Do not use it on private trackers yet: its client ID is not whitelisted anywhere.
>
> Feedback and [bug reports](https://github.com/Balls0fSteel/Rookparcel/issues) are welcome.

**Rookparcel** is a lean BitTorrent client in the spirit of µTorrent 2.2.1: tiny, fast, native, and comfortable with thousands of torrents. Until 1.2.1 it was called PaTorrent; Rookparcel takes over a PaTorrent installation's settings and torrents at its first start.

Website: <https://balls0fsteel.github.io/Rookparcel/>: downloads with how to start each file, the [News](https://balls0fsteel.github.io/Rookparcel/news.html) page (a short changelog per release), an FAQ and links to the documentation.

User guide: the [wiki](https://github.com/Balls0fSteel/Rookparcel/wiki): installing, adding and managing torrents, every Preferences page, ports and VPNs, speed limits, the Web UI, `rookparcel-nox`, Sonarr/Radarr, search, RSS, shortcuts and troubleshooting, with screenshots. Its source is [docs/wiki](docs/wiki).

- **Engine:** our own Rust port of [libtorrent-rasterbar](https://github.com/arvidn/libtorrent) 2.0.11. libtorrent's code is the reference; nothing is linked.
- **Frontends:** a native Win32 GUI (first), a headless daemon `rookparcel-nox` with a Web UI, a GTK 4 GUI for Linux and BSD, and a native AppKit GUI for macOS.
- **Goals:**
  - single self-contained executables
  - minimal dependencies
  - low RAM and CPU use
  - 100,000 torrents on a modest PC, with an instant UI

**Status:** 1.1. The engine downloads and seeds over TCP and uTP, with HTTP, HTTPS and UDP trackers, magnet links, DHT, peer exchange, local peer discovery, encryption, web seeds, proxies, UPnP / NAT-PMP and IP filters. The Win32 GUI has:
- the µTorrent 2.2.1 main window, Preferences, the Add and Create dialogs, RSS and the scheduler;
- since 1.1.7, a Web UI of its own (Preferences > Web UI), which since 1.1.17 includes the RSS Downloader.

`rookparcel-nox` runs headless with its own Web UI and a qBittorrent-compatible API that Sonarr, Radarr and Prowlarr use as a download client. The GTK 4 GUI runs on Linux and BSD; on macOS the native app `Rookparcel.app` replaces the earlier GTK bundle ([docs/MACOS_GUI.md](docs/MACOS_GUI.md)). BitTorrent v2 and hybrid torrents, 34 languages, HTTPS and remote mode are in.

1.1 added a search box for the torrent list and importers for µTorrent and qBittorrent torrents. 1.1.17 adds torrent search through your own Prowlarr or Jackett (Torznab), in all three interfaces, with no search plugins or scrapers of our own; from 1.1.59 the search helper downloads Jackett for you at a click ([docs/SEARCH_ON_WINDOWS.md](docs/SEARCH_ON_WINDOWS.md), the step-by-step guide with screenshots, also on the [wiki](https://github.com/Balls0fSteel/Rookparcel/wiki/Searching-for-torrents-on-Windows)). Validation on more machines continues. See [PLAN.md](PLAN.md), [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/PORTING.md](docs/PORTING.md).

## Download

Release builds are on the [Releases page](https://github.com/Balls0fSteel/Rookparcel/releases); the newest is [1.1.40](https://github.com/Balls0fSteel/Rookparcel/releases/tag/v1.1.40). What changed in each release: the website's [News](https://balls0fsteel.github.io/Rookparcel/news.html) page, or the release notes in [docs/releases](docs/releases). The archives need no installation: unpack and run; Windows also has an installer (from 1.0.1 on). Each release has `SHA256SUMS` and notes with the first-run steps per platform.

| Platform | File | What is inside |
|---|---|---|
| Windows 10 / 11, x64 | `rookparcel-<version>-windows-x64.zip` | `rookparcel.exe` (the GUI) and `rookparcel-nox.exe` (the daemon); statically linked, no runtime needed |
| Windows 10 / 11, x64, installer | `rookparcel-<version>-windows-x64-setup.exe` | the same two programs, installed for you (no administrator rights) or for all users, with a Start menu entry, optional desktop shortcut and `.torrent` / `magnet:` associations, and an uninstaller ([packaging/windows](packaging/windows/README.md)) |
| Linux desktop, x86_64 or aarch64 | `Rookparcel-<version>-<arch>.AppImage` | the GTK 4 GUI with its libraries bundled (glibc 2.39 or newer: Ubuntu 24.04, Fedora 40, Debian 13) |
| Linux desktop, Flatpak | `Rookparcel-<version>-<arch>.flatpak` | the GTK 4 GUI on the GNOME 51 runtime from Flathub |
| Linux, plain binaries | `rookparcel-<version>-linux-<arch>.tar.gz` | `rookparcel-gtk`, `rookparcel-nox`, the desktop entry and the systemd units; they use the system's GTK 4.12+ and OpenSSL 3 |
| Linux server, headless | `rookparcel-nox-<version>-<arch>.AppImage` | `rookparcel-nox` with OpenSSL bundled |
| Docker, amd64 or arm64 | `ghcr.io/balls0fsteel/rookparcel-nox:<version>` (from 1.2.21 on) | `rookparcel-nox` in a container image, for Sonarr, Radarr and Prowlarr setups ([wiki: Docker](https://github.com/Balls0fSteel/Rookparcel/wiki/Docker)) |
| macOS, Apple silicon or Intel | `Rookparcel-<version>-macos.dmg` | `Rookparcel.app`, the native macOS GUI, one universal app (macOS 11 Big Sur or newer; ad-hoc signed, so right-click > Open the first time). Earlier releases have `Rookparcel-<version>-arm64.dmg` instead: the GTK GUI for Apple silicon, macOS 15 or newer |
| macOS, headless | `rookparcel-nox-<version>-macos-<arch>.tar.gz` | `rookparcel-nox` for Apple silicon and Intel |

The builds are not code-signed, so Windows SmartScreen and macOS Gatekeeper warn the first time. How the files are made: [docs/RELEASING.md](docs/RELEASING.md).

**Updates.** From 1.0.1 on, the Windows and GTK GUIs (and the macOS app) check the website's list of releases 10 seconds after they start and every 12 hours. When a newer version is out, the GUI asks once, and a note stays in the status bar until you click it. A Rookparcel installed by the Windows setup offers to download the new setup (checked against the release's `SHA256SUMS`), close, install over itself and start again; a portable copy, the GTK GUI and the macOS app open the release page instead, and nothing is downloaded without your yes. Help > Check for Updates asks at once; Preferences > General > Privacy turns the automatic check off. The Flatpak leaves updating to Flathub. From 1.1.17 on, `rookparcel-nox` checks too: it prints a line and shows a link in its Web UI. `--no-update-check` or the Web UI's General > Privacy setting turns it off.

## Building

Requires Rust 1.85+. On Windows you also need the MSVC build tools (Visual Studio 2022+ "Desktop development with C++"). No other dependencies.

```bash
git clone --recurse-submodules https://github.com/Balls0fSteel/Rookparcel.git
cargo build --release
```

The libtorrent submodule is reference material only and is not needed to build.

On Windows, scripts in the repository root do the same with a double-click (from a prompt, type `.\1_build.bat`: cmd does not run a name that starts with a digit without the `.\`):

| Script | What it does |
|---|---|
| `1_build.bat` | Release build of everything, copied into `dist\` (`1_build.bat debug` for a debug build) |
| `2_run.bat` | Starts `dist\rookparcel.exe` (building first if needed); arguments are passed on |
| `3_build_and_setup.bat` | Release build, then makes the installer and runs it: upgrades the Rookparcel installed on this PC to this code (`/S` for a silent one). Needs NSIS 3 (`makensis`); the setup stays in `dist\` |
| `4_run_demo.bat` | Starts the window on the simulated engine with 2,000 fake torrents (`4_run_demo.bat 10000` for more) |
| `5_fuzz.bat` | Runs a fuzz target (`5_fuzz.bat <target> [seconds]`; nightly Rust, cargo-fuzz and Visual Studio's AddressSanitizer) |

Settings and torrents go to `%APPDATA%\Rookparcel`. An empty `rookparcel.ini` beside the exe keeps them in that folder instead (portable mode).

| Binary | What it is today |
|---|---|
| `rookparcel` | Windows GUI: the µTorrent 2.2.1 main window on the real engine. The arrow beside the toolbar's search box searches your Prowlarr or Jackett ([docs/INTEGRATIONS.md](docs/INTEGRATIONS.md)). `rookparcel --demo 10000` loads 10,000 simulated torrents instead; `rookparcel --remote` (File > Connect to Remote...) drives a `rookparcel-nox` elsewhere through its Web UI port; Preferences > Web UI serves the Web UI from the window itself (the same page nox serves, over this window's torrents, [docs/WEBUI.md](docs/WEBUI.md)). |
| `rookparcel-gtk` | Linux / BSD GUI on GTK 4 (built and tested by CI; run on Linux under Xvfb, not yet used on a desktop): the main window (category list, the torrent list with every column, toolbar, menus, status bar, detail tabs) on the real engine; the dialogs (Add New Torrent, Preferences, Torrent Properties, Create New Torrent, RSS Downloader, Auto Shutdown, Search Torrents and Search Sources); the tray, file associations and autostart; `rookparcel-gtk --remote` (File > Connect to Remote...) drives a `rookparcel-nox` elsewhere. Needs GTK 4.12 or later (`libgtk-4-dev` to build). Settings in `~/.config/rookparcel/gtk`, torrents in `~/.local/share/rookparcel/gtk`. |
| `rookparcel-mac` | macOS GUI on AppKit (`crates/cocoa`), bundled as `Rookparcel.app` for macOS 11 or later, Apple silicon and Intel in one universal app: the same main window, dialogs, tray (a menu-bar status item), notifications, login item and associations as the GTK GUI, with the Mac's menu bar and ⌘ shortcuts ([docs/MACOS_GUI.md](docs/MACOS_GUI.md)). Settings and torrents in `~/Library/Application Support/Rookparcel/mac`; the first start takes over the earlier GTK bundle's `gtk` folder. On macOS, `rookparcel-gtk` is only a stub that points to `Rookparcel.app`. |
| `rookparcel-nox` | Command-line client: `rookparcel-nox add file.torrent --save DIR [--seed]`, `rookparcel-nox run` to resume saved torrents, `rookparcel-nox run --webui 8080 --webui-pass PASS` for the Web UI and a qBittorrent-compatible WebAPI subset that Sonarr, Radarr and Prowlarr can use ([docs/INTEGRATIONS.md](docs/INTEGRATIONS.md); `--webui-https` for HTTPS with a self-signed certificate, or `--webui-cert` / `--webui-key` for yours; `--no-update-check` turns the update check off). As a service (systemd units, a FreeBSD rc.d script, `--daemon`, signals, where its files go): [docs/DAEMON.md](docs/DAEMON.md) |
| `lt-swarm` | Benchmarks: `lt-swarm loopback`, `lt-swarm load --count 10000`, `lt-swarm scale --torrents 100000`, or `lt-swarm gen --out corpus` then `lt-swarm parse corpus`. Interop: `lt-swarm interop` downloads from qBittorrent, Transmission and Deluge (`--encryption`, `--version` v1, hybrid or v2, `--transport` tcp, utp or utp-only), or with `--super-seed` super-seeds to them and reports the copies uploaded (setup: PLAN.md Q10) |

## License

Copyright 2026 DukeCorp1996 (<https://dukecorp1996.x35.in/>).

Rookparcel is free for **noncommercial** use under the [PolyForm Noncommercial License 1.0.0](LICENSE.md): personal use, study, hobby projects, and use by charities, schools, research and public institutions. You may share and modify it on the same terms. Commercial use needs separate permission from DukeCorp1996. This makes Rookparcel source-available rather than open source in the OSI sense.

Code ported from libtorrent remains available under libtorrent's BSD-3-Clause license as well (see below).

## Third-party code and affiliation

Rookparcel is an independent project. It is not affiliated with, endorsed by or sponsored by any of the projects below or their authors.

- **libtorrent-rasterbar** (Arvid Norberg and contributors). The engine is a port of libtorrent 2.0.11, so it is a derivative work and that code stays under libtorrent's BSD-3-Clause license. Every ported source file names its origin and carries libtorrent's copyright notice. [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) has the full license texts (libtorrent and the code bundled with it: puff, ed25519, SHA-1, SHA-256). It ships with every binary distribution next to LICENSE.md, and `rookparcel-nox --license` prints both. As BSD-3-Clause requires, the names of libtorrent and its authors are not used to endorse or promote Rookparcel.
- **qBittorrent.** Rookparcel contains no qBittorrent code, artwork or Web UI. The `nox` Web API follows qBittorrent's published WebAPI documentation so that existing tools can talk to it; it does not copy qBittorrent's (GPL) source, and its small Web UI page is our own. The name appears only to describe compatibility, interop tests and peer-client identification.
- **µTorrent / BitTorrent, Inc.** Rookparcel recreates the µTorrent 2.2.1 look and behaviour but uses none of their code, artwork or trademarks.

Other product names are trademarks of their owners and are used only to identify those products.
