//! BEP 9 ut_metadata messages as a peer sends them, fed into a metadata download: decoding, then
//! storing data blocks with whatever piece index and total size the peer claims.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::InfoHashes;
use patorrent_core::fuzzing::{MetadataFetch, UtMetadataMessage, decode_ut_metadata};

fuzz_target!(|data: &[u8]| {
    let ih = InfoHashes::v1(patorrent_core::crypto::Sha1::digest(b"d4:name1:xe"));
    let mut fetch = MetadataFetch::new();
    // Messages are split on 0xff bytes, so one input can carry several.
    for (i, chunk) in data.split(|&b| b == 0xff).enumerate() {
        if let Ok(UtMetadataMessage::Data { piece, total_size, data }) = decode_ut_metadata(chunk) {
            let _ = fetch.received(i as u64, piece, total_size, data, 1 << 20, &ih);
            let (have, total) = fetch.progress();
            assert!(have <= total);
        }
    }
});
