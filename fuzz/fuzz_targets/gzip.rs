//! gzip members and raw deflate streams (puff): never a panic, the size limit kept, and a
//! stream's reported length within the input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::util::{gzip, puff};

fuzz_target!(|data: &[u8]| {
    if let Ok(out) = gzip::inflate_gzip(data, 1 << 16) {
        assert!(out.len() <= 1 << 16);
    }
    if let Ok((out, used)) = puff::inflate(data, 1 << 16) {
        assert!(out.len() <= 1 << 16);
        assert!(used <= data.len());
    }
});
