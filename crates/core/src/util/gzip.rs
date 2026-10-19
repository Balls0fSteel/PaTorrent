// Ported from libtorrent-rasterbar 2.0.11: src/gzip.cpp, include/libtorrent/gzip.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the output grows up to the limit in one pass instead of re-inflating into a doubled
// buffer; puff's errors keep their own messages (libtorrent maps -9..-11 one code off); like
// libtorrent, the CRC-32 and size trailer are not checked.

//! gzip (RFC 1952) members: the header skipped, the deflate stream inflated with
//! [`puff`](super::puff).

use super::puff::{InflateError, inflate};

const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const FRESERVED: u8 = 0xe0;

/// Why a gzip member could not be inflated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GzipError {
    InvalidHeader,
    Inflate(InflateError),
}

impl std::fmt::Display for GzipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GzipError::InvalidHeader => f.write_str("invalid gzip header"),
            GzipError::Inflate(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for GzipError {}

/// Whether `data` starts like a gzip member (its magic bytes).
pub fn is_gzip(data: &[u8]) -> bool {
    data.starts_with(&[0x1f, 0x8b])
}

/// libtorrent `gzip_header`: the header's length, or `None` when it is not a deflate gzip
/// header.
fn header_len(input: &[u8]) -> Option<usize> {
    if input.len() < 10 || !is_gzip(input) {
        return None;
    }
    let (method, flags) = (input[2], input[3]);
    if method != 8 || flags & FRESERVED != 0 {
        return None;
    }
    let mut rest = &input[10..];
    if flags & FEXTRA != 0 {
        let len = *rest.get(1)? as usize * 256 + *rest.first()? as usize;
        rest = rest.get(2 + len..)?;
    }
    for flag in [FNAME, FCOMMENT] {
        if flags & flag != 0 {
            let end = rest.iter().position(|&b| b == 0)?;
            rest = &rest[end + 1..];
        }
    }
    if flags & FHCRC != 0 {
        rest = rest.get(2..)?;
    }
    Some(input.len() - rest.len())
}

/// libtorrent `inflate_gzip`: the contents of the gzip member `input`, at most `max_size`
/// bytes.
pub fn inflate_gzip(input: &[u8], max_size: usize) -> Result<Vec<u8>, GzipError> {
    let start = header_len(input).ok_or(GzipError::InvalidHeader)?;
    inflate(&input[start..], max_size).map(|(out, _)| out).map_err(GzipError::Inflate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "hello, hello, hello world" with every optional header field (extra, name, comment,
    /// header CRC), as Python's zlib wrote it.
    const FULL: [u8; 61] = [
        31, 139, 8, 30, 0, 0, 0, 0, 0, 255, 3, 0, 97, 98, 99, 110, 97, 109, 101, 46, 116, 120, 116, 0, 97,
        32, 99, 111, 109, 109, 101, 110, 116, 0, 18, 52, 203, 72, 205, 201, 201, 215, 81, 200, 64, 162, 20,
        202, 243, 139, 114, 82, 0, 150, 101, 109, 253, 25, 0, 0, 0,
    ];

    // Ported from libtorrent-rasterbar 2.0.11: test/test_gzip.cpp, with its three files
    #[test]
    fn libtorrent_test_gzip() {
        let zeroes = include_bytes!("../../tests/fixtures/gzip/zeroes.gz");
        let out = inflate_gzip(zeroes, 1_000_000).unwrap();
        assert!(!out.is_empty());
        assert!(out.iter().all(|&b| b == 0));
        assert!(inflate_gzip(include_bytes!("../../tests/fixtures/gzip/corrupt.gz"), 1_000_000).is_err());
        assert!(inflate_gzip(include_bytes!("../../tests/fixtures/gzip/invalid1.gz"), 1_000_000).is_err());
    }

    #[test]
    fn headers_with_every_field() {
        assert_eq!(header_len(&FULL), Some(36));
        assert_eq!(inflate_gzip(&FULL, 100).unwrap(), b"hello, hello, hello world");
        assert_eq!(inflate_gzip(&FULL, 10), Err(GzipError::Inflate(InflateError::TooLarge)));
    }

    #[test]
    fn bad_headers() {
        for n in 0..36 {
            assert_eq!(inflate_gzip(&FULL[..n], 100), Err(GzipError::InvalidHeader), "{n}");
        }
        let mut bad = FULL;
        bad[2] = 7; // not deflate
        assert_eq!(header_len(&bad), None);
        bad = FULL;
        bad[3] |= 0x20; // a reserved flag
        assert_eq!(header_len(&bad), None);
        bad = FULL;
        bad[1] = 0;
        assert!(!is_gzip(&bad));
        assert_eq!(inflate_gzip(b"plain text, not gzip", 100), Err(GzipError::InvalidHeader));
        assert_eq!(inflate_gzip(&FULL[..40], 100), Err(GzipError::Inflate(InflateError::Incomplete)));
        assert_eq!(GzipError::InvalidHeader.to_string(), "invalid gzip header");
    }
}
