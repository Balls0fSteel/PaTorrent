//! NAT-PMP / PCP replies from the gateway.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::parse_natpmp_response;

fuzz_target!(|data: &[u8]| {
    let _ = parse_natpmp_response(data);
});
