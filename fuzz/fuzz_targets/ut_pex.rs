//! BEP 11 ut_pex messages as a peer sends them: decoding into a peer's PEX state (flood
//! limit, caps, local-address filter), then advertising what came in through a torrent's diff
//! and a full list, which must decode again.
#![no_main]

use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::{PeerPex, TorrentPex, encode_ut_pex_full};

fuzz_target!(|data: &[u8]| {
    let t0 = Instant::now();
    let mut rx = PeerPex::default();
    rx.message_id = 1;
    let mut tp = TorrentPex::default();
    let mut all = Vec::new();
    // Messages are split on 0xff bytes; each arrives a minute after the previous one.
    for (i, chunk) in data.split(|&b| b == 0xff).enumerate() {
        let now = t0 + Duration::from_secs(60 * i as u64);
        let remote = if i % 2 == 0 { [10, 0, 0, 1] } else { [9, 9, 9, 9] };
        let Ok(peers) = rx.on_message(chunk, now, remote.into(), 50) else { continue };
        assert!(peers.len() <= 100);
        all.extend(peers);
        tp.tick(now, all.len(), all.iter().copied());
        let mut check = PeerPex::default();
        check.message_id = 1;
        if let Some(diff) = tp.diff() {
            check.on_message(diff, now, [10, 0, 0, 1].into(), 1000).expect("our diff decodes");
        }
        let full = encode_ut_pex_full(all.iter().copied());
        let mut check = PeerPex::default();
        check.message_id = 1;
        check.on_message(&full, now, [10, 0, 0, 1].into(), 1000).expect("our full list decodes");
    }
});
