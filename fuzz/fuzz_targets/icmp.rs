//! Keep ping low's ICMP replies (D49): any host on the path can send them. The input as an echo
//! socket's message (with and without the IPv4 header macOS adds, with and without an identifier
//! to check, from the address pinged and from another), as an entry of Linux's error queue (the
//! quoted packet and the control messages, split at the input's fifth byte), and through the
//! checksum. Reached through `fuzzing::icmp` (`net::probe::fuzz_icmp`), which checks that a reply
//! comes from the address pinged and carries our sequence number.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::icmp;

fuzz_target!(|data: &[u8]| {
    let _ = icmp(data);
});
