//! Mutation testing of the metadata parsers on stable Rust: every fixture is corrupted thousands
//! of ways (bit flips, byte replacements, truncation, splices, length-prefix inflation) and fed to
//! bdecode, `TorrentInfo` and the magnet parser. Nothing may panic; outputs are sanity-checked.
//!
//! This is a deterministic stand-in until coverage-guided fuzz targets run in CI (cargo-fuzz needs
//! a nightly toolchain; see PLAN.md).

use patorrent_core::bencode::{Entry, bdecode, bdecode_partial, print_entry};
use patorrent_core::metainfo::{TorrentInfo, make_magnet_uri, parse_magnet_uri};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

fn fixtures() -> Vec<Vec<u8>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/torrents");
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            (p.display().to_string(), std::fs::read(p).unwrap())
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, b)| b).collect()
}

fn mutate(rng: &mut Rng, input: &[u8]) -> Vec<u8> {
    let mut b = input.to_vec();
    for _ in 0..1 + rng.below(4) {
        if b.is_empty() {
            b.push(b'd');
        }
        let i = rng.below(b.len());
        match rng.below(7) {
            0 => b[i] ^= 1 << rng.below(8),
            1 => b[i] = b"deil0123456789:-"[rng.below(16)],
            2 => b.truncate(i),
            3 => {
                let j = rng.below(b.len());
                let chunk: Vec<u8> = b[i.min(j)..i.max(j)].to_vec();
                let at = rng.below(b.len());
                b.splice(at..at, chunk);
            }
            4 => {
                // inflate a string length prefix
                if let Some(c) = b[i..].iter().position(|&c| c == b':') {
                    b.splice(i + c..i + c, b"9".iter().copied());
                }
            }
            5 => b.insert(i, b"de"[rng.below(2)]),
            _ => {
                b.remove(i);
            }
        }
    }
    b
}

#[test]
fn mutated_torrents_never_panic() {
    let corpus = fixtures();
    // LT_FUZZ_ROUNDS / LT_FUZZ_SEED allow longer local runs (cargo test --release)
    let rounds: usize = std::env::var("LT_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let seed: u64 =
        std::env::var("LT_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x2545_f491_4f6c_dd1d);
    let mut rng = Rng(seed | 1);
    let mut parsed = 0usize;
    for round in 0..rounds {
        for input in &corpus {
            let m = mutate(&mut rng, input);
            if let Ok(ti) = TorrentInfo::from_bytes(&m) {
                parsed += 1;
                // everything a frontend would touch must be consistent
                let fs = ti.files();
                assert!(fs.num_files() > 0);
                let mut total = 0;
                for i in 0..fs.num_files() {
                    let _ = fs.file_path(i, "save");
                    let _ = fs.symlink(i);
                    total += fs.file_size(i);
                }
                assert_eq!(total, fs.total_size());
                assert!(ti.num_pieces() > 0);
                if ti.v1() {
                    assert!(ti.hash_for_piece(ti.num_pieces() - 1).is_some());
                }
                let _ = fs.map_block(ti.num_pieces() - 1, 0, ti.piece_size(ti.num_pieces() - 1) as u64);
            }
            // the raw decoder, including partial-tree printing
            let (tree, _) = bdecode_partial(&m[..], 100, 100_000);
            if let Some(root) = tree.root() {
                let _ = print_entry(root, true, 0);
                let _ = root.has_soft_error();
                let _ = Entry::from_node(root);
            }
        }
        let _ = round;
    }
    // sanity: the mutations should not be so destructive that nothing ever parses
    assert!(parsed > 100, "only {parsed} mutated torrents parsed");
}

#[test]
fn entry_roundtrips_through_bdecode() {
    for input in fixtures() {
        if let Ok(tree) = bdecode(&input[..]) {
            let root = tree.root().unwrap();
            if root.has_soft_error().is_none() {
                // canonical encodings re-encode byte for byte
                assert_eq!(Entry::from_node(root).bencode(), root.data_section());
            }
        }
    }
}

#[test]
fn mutated_magnets_never_panic() {
    let seeds = [
        "magnet:?xt=urn:btih:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd&dn=foo&tr=udp%3A%2F%2Fx%3A1&ws=http://a/b&so=0,2-4&x.pe=[::1]:45&dht=h:1",
        "magnet:?xt=urn:btmh:1220cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd&xt=urn:btih:MFRGCYTBMJQWEYLCMFRGCYTBMJQWEYLC",
    ];
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..5000 {
        let seed = seeds[rng.below(seeds.len())].as_bytes();
        let m = mutate(&mut rng, seed);
        let s = String::from_utf8_lossy(&m);
        if let Ok(p) = parse_magnet_uri(&s) {
            // whatever parses must survive a round trip
            let again = parse_magnet_uri(&make_magnet_uri(&p)).expect("round trip");
            assert_eq!(again.info_hashes, p.info_hashes);
            assert_eq!(again.trackers, p.trackers);
        }
    }
}
