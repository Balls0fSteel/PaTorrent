//! HTTP responses from trackers and web seeds, and redirect resolution against a parsed URL.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::{Url, parse_response, resolve_redirect};

fuzz_target!(|data: &[u8]| {
    let _ = parse_response(data, false);
    let _ = parse_response(data, true);
    if let Ok(s) = std::str::from_utf8(data) {
        if let Some(base) = Url::parse("http://tracker.example:6969/announce") {
            let _ = resolve_redirect(&base, s);
        }
        let _ = Url::parse(s);
    }
});
