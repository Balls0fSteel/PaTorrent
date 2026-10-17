//! Part file headers (`.<info-hash>.parts`, read back from disk): parsing with the torrent's
//! piece count and size, or with the counts the header itself claims, then the slot map's
//! invariants (every slot below the end is free or held by one piece), a serialize / parse
//! round trip (of the trimmed map), and a few allocations and frees driven by the input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::PartFileSlotMap as SlotMap;

fn check(m: &SlotMap, pieces: u32, piece_size: u32) {
    assert!(m.is_consistent());
    let h = m.serialize();
    assert_eq!(h.len() as u64, SlotMap::header_size(pieces));
    // Free slots past the last used one are not in the header (`flush_metadata` trims them
    // before writing), so the header reads back as the trimmed map.
    let mut trimmed = m.clone();
    trimmed.trim();
    assert_eq!(SlotMap::parse(&h, pieces, piece_size), trimmed);
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 {
        return;
    }
    // A small torrent from the first bytes, so headers of the right size turn up.
    let pieces = 1 + u16::from_le_bytes([data[0], data[1]]) as u32 % 2048;
    let piece_size = 0x4000u32 << (data[2] % 8);
    let header = &data[3..];
    let mut m = SlotMap::parse(header, pieces, piece_size);
    check(&m, pieces, piece_size);

    // The header's own counts, as a corrupt file might claim (capped to keep allocations small).
    if header.len() >= 8 {
        let claimed = u32::from_be_bytes(header[..4].try_into().unwrap());
        let size = u32::from_be_bytes(header[4..8].try_into().unwrap());
        if (1..=1 << 16).contains(&claimed) {
            check(&SlotMap::parse(header, claimed, size), claimed, size);
        }
    }

    for &b in header.iter().rev().take(64) {
        let piece = b as u32 % pieces;
        if b & 1 == 0 {
            let slot = m.allocate(piece);
            assert_eq!(m.slot(piece), Some(slot));
        } else {
            m.free(piece);
            assert_eq!(m.slot(piece), None);
        }
        if b & 0x80 != 0 {
            m.trim();
        }
        assert!(m.is_consistent());
    }
    check(&m, pieces, piece_size);
});
