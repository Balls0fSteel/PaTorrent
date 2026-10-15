//! bdecode, its partial variant and the printer must never panic on arbitrary input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use libretorrent_core::bencode::{bdecode, bdecode_partial, print_entry};

fuzz_target!(|data: &[u8]| {
    if let Ok(d) = bdecode(data)
        && let Some(root) = d.root()
    {
        let _ = print_entry(root, true, 0);
    }
    // libtorrent's default limits
    let (partial, _) = bdecode_partial(data, 100, 2_000_000);
    if let Some(root) = partial.root() {
        let _ = print_entry(root, false, 0);
    }
});
