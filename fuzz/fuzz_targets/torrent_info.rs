//! .torrent parsing: v1, v2 and hybrid metadata from untrusted files and peers (BEP 9).
#![no_main]

use libfuzzer_sys::fuzz_target;
use libretorrent_core::metainfo::TorrentInfo;

fuzz_target!(|data: &[u8]| {
    let _ = TorrentInfo::from_bytes(data);
});
