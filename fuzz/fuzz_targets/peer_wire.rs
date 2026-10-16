//! Peer wire input as it arrives from a remote peer: the handshake, then a stream of messages,
//! including BEP 10 extension handshakes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::{
    PeerWireMessage, decode, decode_dont_have, decode_ext_handshake, decode_handshake, identify_client,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(h) = decode_handshake(data) {
        let _ = identify_client(&h.peer_id);
    }
    let _ = decode_ext_handshake(data);
    let mut buf = data;
    while let Ok(Some((m, used))) = decode(buf) {
        assert!(used > 0 && used <= buf.len());
        if let PeerWireMessage::Extended { payload, .. } = m {
            let _ = decode_dont_have(payload);
        }
        buf = &buf[used..];
    }
});
