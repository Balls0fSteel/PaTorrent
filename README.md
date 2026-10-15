# LibreTorrent

A lean, libre BitTorrent client in the spirit of µTorrent 2.2.1: tiny, fast, native, and comfortable with thousands of torrents.

- **Engine:** our own Rust port of [libtorrent-rasterbar](https://github.com/arvidn/libtorrent) 2.0.11. libtorrent's code is the reference; nothing is linked.
- **Frontends:** a native Win32 GUI (first), a headless daemon `libretorrent-nox` with a Web UI, and a GTK GUI for Linux/BSD.
- **Goals:**
  - single self-contained executables
  - minimal dependencies
  - low RAM and CPU use
  - instant UI with 10,000+ torrents

**Status:** planning. See [PLAN.md](PLAN.md), [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/PORTING.md](docs/PORTING.md).

LibreTorrent is not affiliated with µTorrent or BitTorrent, Inc. It uses none of their code, artwork or trademarks.
