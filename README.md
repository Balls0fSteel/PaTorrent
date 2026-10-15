# PaTorrent

**PaTorrent** (long name **PatinaTorrent**) is a lean BitTorrent client in the spirit of µTorrent 2.2.1: tiny, fast, native, and comfortable with thousands of torrents. A patina is the good kind of rust.

- **Engine:** our own Rust port of [libtorrent-rasterbar](https://github.com/arvidn/libtorrent) 2.0.11. libtorrent's code is the reference; nothing is linked.
- **Frontends:** a native Win32 GUI (first), a headless daemon `patorrent-nox` with a Web UI, and a GTK GUI for Linux/BSD.
- **Goals:**
  - single self-contained executables
  - minimal dependencies
  - low RAM and CPU use
  - instant UI with 10,000+ torrents

**Status:** Phase 2 is built: the real engine downloads and seeds `.torrent` files over TCP with HTTP and UDP trackers, and resumes after a restart without rechecking. Magnet links, DHT, encryption, uTP and HTTPS trackers come next. See [PLAN.md](PLAN.md), [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/PORTING.md](docs/PORTING.md).

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
| `patorrent` | Windows GUI: the µTorrent 2.2.1 main window on the real engine (Phase 3, in progress). `patorrent --demo 10000` loads 10,000 simulated torrents instead. |
| `patorrent-nox` | Command-line client: `patorrent-nox add file.torrent --save DIR [--seed]`, `patorrent-nox run` to resume saved torrents |
| `lt-swarm` | Benchmarks: `lt-swarm loopback`, `lt-swarm load --count 10000`, `lt-swarm scale --torrents 100000`, or `lt-swarm gen --out corpus` then `lt-swarm parse corpus`. Interop: `lt-swarm interop` downloads from qBittorrent, Transmission and Deluge (setup: PLAN.md Q10) |

## License

Copyright 2026 DukeCorp1996 (<https://dukecorp1996.x35.in/>).

PaTorrent is free for **noncommercial** use under the [PolyForm Noncommercial License 1.0.0](LICENSE.md): personal use, study, hobby projects, and use by charities, schools, research and public institutions. You may share and modify it on the same terms. Commercial use needs separate permission from DukeCorp1996. This makes PaTorrent source-available rather than open source in the OSI sense.

Code ported from libtorrent remains available under libtorrent's BSD-3-Clause license as well (see below).

## Third-party code and affiliation

PaTorrent is an independent project. It is not affiliated with, endorsed by or sponsored by any of the projects below or their authors.

- **libtorrent-rasterbar** (Arvid Norberg and contributors). The engine is a port of libtorrent 2.0.11, so it is a derivative work and that code stays under libtorrent's BSD-3-Clause license. Every ported source file names its origin and carries libtorrent's copyright notice. [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) has the full license texts (libtorrent and the code bundled with it: puff, ed25519, SHA-1, SHA-256). It ships with every binary distribution next to LICENSE.md, and `patorrent-nox --license` prints both. As BSD-3-Clause requires, the names of libtorrent and its authors are not used to endorse or promote PaTorrent.
- **qBittorrent.** PaTorrent contains no qBittorrent code, artwork or Web UI. The `nox` Web API will follow qBittorrent's published WebAPI documentation so that existing tools can talk to it; it will not copy qBittorrent's (GPL) source. The name appears only to describe compatibility, interop tests and peer-client identification.
- **µTorrent / BitTorrent, Inc.** PaTorrent recreates the µTorrent 2.2.1 look and behaviour but uses none of their code, artwork or trademarks.

Other product names are trademarks of their owners and are used only to identify those products.
