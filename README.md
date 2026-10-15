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
| `lt-swarm` | Benchmarks: `lt-swarm loopback`, `lt-swarm load --count 10000`, `lt-swarm scale --torrents 100000`, or `lt-swarm gen --out corpus` then `lt-swarm parse corpus` |

LibreTorrent is not affiliated with µTorrent or BitTorrent, Inc. It uses none of their code, artwork or trademarks.
