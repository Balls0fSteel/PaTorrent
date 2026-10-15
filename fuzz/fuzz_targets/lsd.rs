//! LSD (BEP 14) datagrams: parsing, and re-parsing the announce we would send for what was read.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::{parse_lsd, render_lsd};

fuzz_target!(|data: &[u8]| {
    if let Ok(a) = parse_lsd(data) {
        for ih in &a.info_hashes {
            let p = render_lsd(a.port, ih, a.cookie.unwrap_or(0) as u32, a.port % 2 == 0);
            let b = parse_lsd(&p).expect("our own announce parses");
            assert_eq!((b.port, &b.info_hashes[..]), (a.port, std::slice::from_ref(ih)));
        }
    }
});
