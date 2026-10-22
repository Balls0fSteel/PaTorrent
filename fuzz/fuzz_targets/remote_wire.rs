//! The remote protocol (`core::remote`, the GUIs' remote mode on nox's Web UI port): the input
//! as one frame through every decoder (commands, alerts, the hello, and the typed values on
//! their own), then split into length-prefixed frames, each through them again. Whatever
//! decodes must re-encode to bytes that survive a second round trip unchanged.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::remote_wire;

fuzz_target!(|data: &[u8]| {
    let _ = remote_wire(data);
});
