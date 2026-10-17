//! Web seed responses (BEP 19 and BEP 17): arbitrary bytes from the "server", fed in pieces of
//! arbitrary sizes, after a few range requests on a multi-file torrent with pad files or on a
//! single file. Blocks must come out whole, in request order. Also the header and chunk parsers
//! and the redirect helpers on their own.
#![no_main]

use std::collections::HashMap;

use libfuzzer_sys::fuzz_target;
use patorrent_core::Sha1Hash;
use patorrent_core::fuzzing::{
    WebEvent, WebPeer, parse_chunk_header, parse_http_head, resolve_redirect_location, split_url,
};
use patorrent_core::metainfo::{FileFlags, FileStorage, WebSeedKind};

fn storage(single: bool) -> FileStorage {
    let mut fs = FileStorage::new();
    if single {
        fs.add_file("one", 100_000, FileFlags::empty(), 0, "", None).unwrap();
    } else {
        for (i, (size, pad)) in [(5u64, false), (16379, true), (20_000, false), (0, false), (12_388, true), (40_000, false)]
            .into_iter()
            .enumerate()
        {
            let flags = if pad { FileFlags::PAD_FILE } else { FileFlags::empty() };
            fs.add_file(&format!("t/{i}"), size, flags, 0, "", None).unwrap();
        }
    }
    fs.set_piece_length(16 * 1024);
    fs.set_num_pieces(fs.total_size().div_ceil(16 * 1024) as u32);
    fs
}

fuzz_target!(|data: &[u8]| {
    let _ = parse_http_head(data);
    let _ = parse_chunk_header(data);
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = resolve_redirect_location("http://a.example/b/c", s);
        let _ = split_url(s);
    }
    let Some((&mode, rest)) = data.split_first() else { return };
    let fs = storage(mode & 1 != 0);
    let kind = if mode & 2 != 0 { WebSeedKind::HttpSeed } else { WebSeedKind::UrlSeed };
    let mut p = WebPeer::new("http://a.example/x/", kind, Sha1Hash([1; 20]), "", &[], &fs, "f", false).unwrap();
    // requests: whole pieces from `mode >> 2`, each one piece long (BEP 17 needs that)
    let mut want = Vec::new();
    let first = (mode >> 2) as u32 % fs.num_pieces();
    for piece in first..fs.num_pieces().min(first + 3) {
        let len = fs.piece_size(piece);
        p.write_request(&fs, piece, 0, len, &HashMap::new(), &mut None);
        for b in 0..len.div_ceil(16384) {
            want.push((piece, b * 16384, (len - b * 16384).min(16384)));
        }
    }
    let mut got = Vec::new();
    let mut check = |p: &mut WebPeer| {
        for e in p.take_events() {
            if let WebEvent::Block { piece, start, data } = e {
                got.push((piece, start, data.len() as u32));
            }
        }
    };
    check(&mut p);
    // the rest: chunk lengths from each byte, until the data runs out
    let mut i = 0;
    let mut step = 1;
    while i < rest.len() {
        let n = step.min(rest.len() - i);
        p.feed(&fs, &rest[i..i + n]);
        check(&mut p);
        i += n;
        step = (rest[i - 1] as usize % 61) + 1;
    }
    assert!(got.len() <= want.len());
    assert_eq!(got[..], want[..got.len()], "blocks whole and in order");
    let _ = p.take_restart();
});
