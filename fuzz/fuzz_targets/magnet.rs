//! Magnet URIs: parsing, and re-parsing what `make_magnet_uri` writes back.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::metainfo::{make_magnet_uri, parse_magnet_uri};

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    if let Ok(m) = parse_magnet_uri(s) {
        let _ = parse_magnet_uri(&make_magnet_uri(&m));
    }
});
