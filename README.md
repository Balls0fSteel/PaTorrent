# LibreTorrent

A lean, libre BitTorrent client in the spirit of µTorrent 2.2.1: tiny, fast, native, and comfortable with thousands of torrents.

- **Engine:** our own Rust port of [libtorrent-rasterbar](https://github.com/arvidn/libtorrent) 2.0.11. libtorrent's code is the reference; nothing is linked.
- **Frontends:** a native Win32 GUI (first), a headless daemon `libretorrent-nox` with a Web UI, and a GTK GUI for Linux/BSD.
- **Goals:**
  - single self-contained executables
  - minimal dependencies
  - low RAM and CPU use
  - instant UI with 10,000+ torrents

**Status:** Phase 2 is built: the real engine downloads and seeds `.torrent` files over TCP with HTTP and UDP trackers, and resumes after a restart without rechecking. Magnet links, DHT, encryption, uTP and HTTPS trackers come next. See [PLAN.md](PLAN.md), [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/PORTING.md](docs/PORTING.md).

## Building

Requires Rust 1.85+. On Windows you also need the MSVC build tools (Visual Studio 2022+ "Desktop development with C++"). No other dependencies.

```bash
git clone --recurse-submodules https://github.com/Balls0fSteel/LibreTorrent.git
cargo build --release
```

The libtorrent submodule is reference material only and is not needed to build.

| Binary | What it is today |
|---|---|
| `libretorrent` | Windows GUI skeleton. Run `libretorrent --demo 10000` to load 10,000 simulated torrents. |
| `libretorrent-nox` | Command-line client: `libretorrent-nox add file.torrent --save DIR [--seed]`, `libretorrent-nox run` to resume saved torrents |
| `lt-swarm` | Benchmarks: `lt-swarm loopback`, `lt-swarm load --count 10000`, `lt-swarm scale --torrents 100000`, or `lt-swarm gen --out corpus` then `lt-swarm parse corpus`. Interop: `lt-swarm interop` downloads from qBittorrent, Transmission and Deluge (setup: PLAN.md Q10) |

## Third-party code and affiliation

LibreTorrent is an independent project. It is not affiliated with, endorsed by or sponsored by any of the projects below or their authors.

- **libtorrent-rasterbar** (Arvid Norberg and contributors). The engine is a port of libtorrent 2.0.11, so it is a derivative work and that code stays under libtorrent's BSD-3-Clause license. Every ported source file names its origin and carries libtorrent's copyright notice. [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) has the full license texts (libtorrent and the code bundled with it: puff, ed25519, SHA-1, SHA-256). It ships with every binary distribution, and `libretorrent-nox --license` prints it. As BSD-3-Clause requires, the names of libtorrent and its authors are not used to endorse or promote LibreTorrent.
- **qBittorrent.** LibreTorrent contains no qBittorrent code, artwork or Web UI. The `nox` Web API will follow qBittorrent's published WebAPI documentation so that existing tools can talk to it; it will not copy qBittorrent's (GPL) source. The name appears only to describe compatibility, interop tests and peer-client identification.
- **µTorrent / BitTorrent, Inc.** LibreTorrent recreates the µTorrent 2.2.1 look and behaviour but uses none of their code, artwork or trademarks.

Other product names are trademarks of their owners and are used only to identify those products.
