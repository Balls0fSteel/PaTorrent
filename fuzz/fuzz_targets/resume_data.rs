//! Resume data (docs/PORTING.md rule 8): a fast-resume record through `ResumeData::decode`, with
//! an encode / decode round trip of whatever decodes, and the store's start-up read of
//! `resume.db` (the bencoded dictionary of every record) and `resume.log` (the journal replayed
//! over it), the input split in two by its first two bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::{resume_data_round_trip, store_records};

fuzz_target!(|data: &[u8]| {
    resume_data_round_trip(data);
    if data.len() >= 2 {
        let rest = &data[2..];
        let split = (u16::from_le_bytes([data[0], data[1]]) as usize).min(rest.len());
        let (db, log) = rest.split_at(split);
        store_records(db, log);
        store_records(b"", rest);
    }
});
