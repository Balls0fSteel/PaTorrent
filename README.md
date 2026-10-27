# PaTorrent

> [!WARNING]
> **Release candidate. Pre-release software.**
>
> PaTorrent 1.0.0-rc.1 is the first public build. It works for the author, but it has had no wider testing: settings and saved state may still change between versions, some features are only half tested, and nothing is code-signed. Do not use it for torrents you care about, and do not use it on private trackers: its client ID is not whitelisted anywhere yet.
>
> Feedback and [bug reports](https://github.com/Balls0fSteel/PaTorrent/issues) are welcome.

**PaTorrent** (long name **PatinaTorrent**) is a lean BitTorrent client in the spirit of µTorrent 2.2.1: tiny, fast, native, and comfortable with thousands of torrents. A patina is the good kind of rust.

- **Engine:** our own Rust port of [libtorrent-rasterbar](https://github.com/arvidn/libtorrent) 2.0.11. libtorrent's code is the reference; nothing is linked.
- **Frontends:** a native Win32 GUI (first), a headless daemon `patorrent-nox` with a Web UI, and a GTK 4 GUI for Linux, BSD and macOS.
- **Goals:**
  - single self-contained executables
  - minimal dependencies
  - low RAM and CPU use
  - 100,000 torrents on a modest PC, with an instant UI

**Status:** release candidate, working toward 1.0 (see "Road to 1.0" in [PLAN.md](PLAN.md)). The engine downloads and seeds over TCP and uTP, with HTTP, HTTPS and UDP trackers, magnet links, DHT, peer exchange, local peer discovery, encryption, web seeds, proxies, UPnP / NAT-PMP and IP filters. The Win32 GUI has the µTorrent 2.2.1 main window, Preferences, the Add and Create dialogs, RSS and the scheduler; `patorrent-nox` runs headless with its own Web UI and a qBittorrent-compatible API. The GTK 4 GUI runs on Linux, BSD and macOS. BitTorrent v2 and hybrid torrents, 34 languages, HTTPS and remote mode are in; what remains before 1.0 is validation on more machines. See [PLAN.md](PLAN.md), [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/PORTING.md](docs/PORTING.md).

## Download

Release builds are on the [Releases page](https://github.com/Balls0fSteel/PaTorrent/releases); the newest is [1.0.0-rc.1](https://github.com/Balls0fSteel/PaTorrent/releases/tag/v1.0.0-rc.1). The archives need no installation: unpack and run; Windows also has an installer (from the release after 1.0.0-rc.1). Each release has `SHA256SUMS` and notes with the first-run steps per platform.

| Platform | File | What is inside |
|---|---|---|
| Windows 10 / 11, x64 | `patorrent-<version>-windows-x64.zip` | `patorrent.exe` (the GUI) and `patorrent-nox.exe` (the daemon); statically linked, no runtime needed |
| Windows 10 / 11, x64, installer | `patorrent-<version>-windows-x64-setup.exe` | the same two programs, installed for you (no administrator rights) or for all users, with a Start menu entry, optional desktop shortcut and `.torrent` / `magnet:` associations, and an uninstaller ([packaging/windows](packaging/windows/README.md)) |
| Linux desktop, x86_64 or aarch64 | `PaTorrent-<version>-<arch>.AppImage` | the GTK 4 GUI with its libraries bundled (glibc 2.39 or newer: Ubuntu 24.04, Fedora 40, Debian 13) |
| Linux desktop, Flatpak | `PaTorrent-<version>-<arch>.flatpak` | the GTK 4 GUI on the GNOME 51 runtime from Flathub |
| Linux, plain binaries | `patorrent-<version>-linux-<arch>.tar.gz` | `patorrent-gtk`, `patorrent-nox`, the desktop entry and the systemd units; they use the system's GTK 4.12+ and OpenSSL 3 |
| Linux server, headless | `patorrent-nox-<version>-<arch>.AppImage` | `patorrent-nox` with OpenSSL bundled |
| macOS, Apple silicon | `PaTorrent-<version>-arm64.dmg` | `PaTorrent.app` (macOS 15 or newer; ad-hoc signed, so right-click > Open the first time) |
| macOS, headless | `patorrent-nox-<version>-macos-<arch>.tar.gz` | `patorrent-nox` for Apple silicon and Intel |

The builds are not code-signed, so Windows SmartScreen and macOS Gatekeeper warn the first time. How the files are made: [docs/RELEASING.md](docs/RELEASING.md).

## Building

Requires Rust 1.85+. On Windows you also need the MSVC build tools (Visual Studio 2022+ "Desktop development with C++"). No other dependencies.

```bash
git clone --recurse-submodules https://github.com/Balls0fSteel/PaTorrent.git
cargo build --release
```

The libtorrent submodule is reference material only and is not needed to build.

On Windows, three scripts in the repository root do the same with a double-click:

| Script | What it does |
|---|---|
| `build.bat` | Release build of everything, copied into `dist\` (`build.bat debug` for a debug build) |
| `run.bat` | Starts `dist\patorrent.exe` (building first if needed); arguments are passed on |
| `run-demo.bat` | Starts the window on the simulated engine with 2,000 fake torrents (`run-demo.bat 10000` for more) |

Settings and torrents go to `%APPDATA%\PaTorrent`. An empty `patorrent.ini` beside the exe keeps them in that folder instead (portable mode).

| Binary | What it is today |
|---|---|
| `patorrent` | Windows GUI: the µTorrent 2.2.1 main window on the real engine (Phase 3, in progress). `patorrent --demo 10000` loads 10,000 simulated torrents instead; `patorrent --remote` (File > Connect to Remote...) drives a `patorrent-nox` elsewhere through its Web UI port. |
| `patorrent-gtk` | Linux / BSD GUI on GTK 4 (Phase 7; not yet run on Linux, built by CI): the main window (category list, the torrent list with every column, toolbar, menus, status bar, detail tabs) on the real engine; the dialogs (Add New Torrent, Preferences, Torrent Properties, Create New Torrent, RSS Downloader, Auto Shutdown); the tray, file associations and autostart; `patorrent-gtk --remote` (File > Connect to Remote...) drives a `patorrent-nox` elsewhere. Needs GTK 4.12 or later (`libgtk-4-dev` to build). Settings in `~/.config/patorrent/gtk`, torrents in `~/.local/share/patorrent/gtk`. |
| `patorrent-nox` | Command-line client: `patorrent-nox add file.torrent --save DIR [--seed]`, `patorrent-nox run` to resume saved torrents, `patorrent-nox run --webui 8080 --webui-pass PASS` for the Web UI and a qBittorrent-compatible WebAPI subset (`--webui-https` for HTTPS with a self-signed certificate, or `--webui-cert` / `--webui-key` for yours). As a service (systemd units, a FreeBSD rc.d script, `--daemon`, signals, where its files go): [docs/DAEMON.md](docs/DAEMON.md) |
| `lt-swarm` | Benchmarks: `lt-swarm loopback`, `lt-swarm load --count 10000`, `lt-swarm scale --torrents 100000`, or `lt-swarm gen --out corpus` then `lt-swarm parse corpus`. Interop: `lt-swarm interop` downloads from qBittorrent, Transmission and Deluge (`--encryption`, `--version` v1, hybrid or v2, `--transport` tcp, utp or utp-only), or with `--super-seed` super-seeds to them and reports the copies uploaded (setup: PLAN.md Q10) |

## License

Copyright 2026 DukeCorp1996 (<https://dukecorp1996.x35.in/>).

PaTorrent is free for **noncommercial** use under the [PolyForm Noncommercial License 1.0.0](LICENSE.md): personal use, study, hobby projects, and use by charities, schools, research and public institutions. You may share and modify it on the same terms. Commercial use needs separate permission from DukeCorp1996. This makes PaTorrent source-available rather than open source in the OSI sense.

Code ported from libtorrent remains available under libtorrent's BSD-3-Clause license as well (see below).

## Third-party code and affiliation

PaTorrent is an independent project. It is not affiliated with, endorsed by or sponsored by any of the projects below or their authors.

- **libtorrent-rasterbar** (Arvid Norberg and contributors). The engine is a port of libtorrent 2.0.11, so it is a derivative work and that code stays under libtorrent's BSD-3-Clause license. Every ported source file names its origin and carries libtorrent's copyright notice. [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) has the full license texts (libtorrent and the code bundled with it: puff, ed25519, SHA-1, SHA-256). It ships with every binary distribution next to LICENSE.md, and `patorrent-nox --license` prints both. As BSD-3-Clause requires, the names of libtorrent and its authors are not used to endorse or promote PaTorrent.
- **qBittorrent.** PaTorrent contains no qBittorrent code, artwork or Web UI. The `nox` Web API follows qBittorrent's published WebAPI documentation so that existing tools can talk to it; it does not copy qBittorrent's (GPL) source, and its small Web UI page is our own. The name appears only to describe compatibility, interop tests and peer-client identification.
- **µTorrent / BitTorrent, Inc.** PaTorrent recreates the µTorrent 2.2.1 look and behaviour but uses none of their code, artwork or trademarks.

Other product names are trademarks of their owners and are used only to identify those products.
