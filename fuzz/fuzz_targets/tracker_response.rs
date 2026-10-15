//! HTTP tracker announce and scrape bodies, and compact peer lists (BEP 23, BEP 7).
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::Sha1Hash;
use patorrent_core::fuzzing::{parse_announce_response, parse_compact_peers, parse_scrape_response};

fuzz_target!(|data: &[u8]| {
    let _ = parse_announce_response(data);
    let _ = parse_scrape_response(data, &Sha1Hash::default());
    let mut peers = Vec::new();
    parse_compact_peers(data, false, &mut peers);
    parse_compact_peers(data, true, &mut peers);
});
