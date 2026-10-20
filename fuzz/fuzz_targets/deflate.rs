//! The deflate encoder: whatever goes in comes back out of puff unchanged, and the stream
//! ends where puff says it does.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::util::{deflate, puff};

fuzz_target!(|data: &[u8]| {
    let packed = deflate::deflate(data);
    let (out, used) = puff::inflate(&packed, data.len()).expect("our own stream inflates");
    assert!(out == data);
    assert_eq!(used, packed.len());
});
