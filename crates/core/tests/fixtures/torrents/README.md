# Test torrents

Copied unchanged from libtorrent-rasterbar 2.0.11, `test/test_torrents/`
(https://github.com/arvidn/libtorrent, tag `v2.0.11`). They are libtorrent's own parser fixtures,
valid and deliberately broken, and are used here so the Rust port is held to the same expectations
(`crates/core/src/metainfo/torrent_info.rs` tests and `crates/core/tests/robustness.rs`).
`v2_empty_filename.torrent` and `v2_invalid_filename2.torrent` come from upstream commit
2b2929af7d (2.0.12, "fix handling of invalid filenames").

Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause; see THIRD-PARTY-NOTICES.md
in the repository root.
