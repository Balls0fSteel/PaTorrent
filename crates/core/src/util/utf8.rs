// Ported from libtorrent-rasterbar 2.0.11: src/utf8.cpp, and `aux::verify_encoding` from
// src/torrent_info.cpp.
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: none in behaviour; codepoints are `Option<u32>` instead of -1.

//! UTF-8 codepoint parsing that tolerates and reports broken input (metadata is untrusted and
//! often not valid UTF-8).

const MAX_CODEPOINT: u32 = 0x10ffff;
const SURROGATES: std::ops::RangeInclusive<u32> = 0xd800..=0xdfff;

/// Bytes in the sequence that starts with `b`; 0 for an invalid lead byte. 5 is returned for
/// the (invalid) 5-byte prefix so it can be skipped as a unit.
fn sequence_length(b: u8) -> usize {
    if b < 0b1000_0000 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else if b >> 2 == 0b111110 {
        5
    } else {
        0
    }
}

/// Parses one codepoint from the start of `s`. Returns the codepoint (`None` if invalid) and the
/// number of bytes consumed (at least 1 for non-empty input).
pub fn parse_utf8_codepoint(s: &[u8]) -> (Option<u32>, usize) {
    let Some(&lead) = s.first() else { return (None, 0) };
    let len = sequence_length(lead);
    match len {
        1 => return (Some(lead as u32), 1),
        0 => return (None, 1),
        5.. => return (None, len),
        _ if len > s.len() => return (None, s.len()),
        _ => {}
    }
    let mut ch = (lead & (0xff >> (len + 1))) as u32;
    for &b in &s[1..len] {
        if !(0b1000_0000..=0b1011_1111).contains(&b) {
            return (None, len);
        }
        ch = (ch << 6) | (b & 0b11_1111) as u32;
    }
    let overlong = match len {
        2 => ch < 0x80,
        3 => ch < 0x800,
        _ => ch < 0x10000,
    };
    if overlong || ch > MAX_CODEPOINT || SURROGATES.contains(&ch) {
        return (None, len);
    }
    (Some(ch), len)
}

/// Appends `cp` as UTF-8; surrogates and out-of-range values become '_'.
pub fn append_utf8_codepoint(out: &mut Vec<u8>, cp: u32) {
    let cp = if SURROGATES.contains(&cp) || cp > MAX_CODEPOINT { '_' as u32 } else { cp };
    let c = char::from_u32(cp).expect("validated above");
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// libtorrent `aux::verify_encoding`: returns valid UTF-8, replacing each invalid sequence with
/// '_'. The flag reports whether the input was already valid.
pub fn verify_encoding(s: &[u8]) -> (String, bool) {
    if let Ok(v) = std::str::from_utf8(s) {
        return (v.to_owned(), true);
    }
    let mut out = Vec::with_capacity(s.len() + 5);
    let mut rest = s;
    while !rest.is_empty() {
        let (cp, len) = parse_utf8_codepoint(rest);
        append_utf8_codepoint(&mut out, cp.unwrap_or('_' as u32));
        rest = &rest[len.min(rest.len())..];
    }
    (String::from_utf8(out).expect("only valid codepoints were appended"), false)
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_utf8.cpp.
    use super::*;

    fn roundtrip(cp: u32) {
        let mut utf8 = Vec::new();
        append_utf8_codepoint(&mut utf8, cp);
        assert_eq!(parse_utf8_codepoint(&utf8), (Some(cp), utf8.len()));
    }

    #[test]
    fn parse_utf8_roundtrip() {
        (0..0xd800).for_each(roundtrip);
        // surrogates are invalid and don't round-trip
        (0xe000..0xffff).for_each(roundtrip);
    }

    #[test]
    fn parse_utf8() {
        for s in [
            &b"\x7f"[..],
            b"\xc3\xb0",
            b"\xed\x9f\xbf",
            b"\xee\x80\x80",
            b"\xef\xbf\xbd",
            b"\xf4\x8f\xbf\xbf",
        ] {
            let (cp, len) = parse_utf8_codepoint(s);
            assert_eq!(len, s.len());
            let mut out = Vec::new();
            append_utf8_codepoint(&mut out, cp.unwrap());
            assert_eq!(out, s);
        }
    }

    #[test]
    fn parse_utf8_fail() {
        let cases: &[&[u8]] = &[
            // unexpected continuation bytes
            b"\x80",
            b"\xbf",
            // impossible bytes
            b"\xff",
            b"\xfe",
            b"\xff\xff\xfe\xfe",
            // overlong ASCII
            b"\xc0\xaf",
            b"\xe0\x80\xaf",
            b"\xf0\x80\x80\xaf",
            b"\xf8\x80\x80\x80\xaf ",
            b"\xfc\x80\x80\x80\x80\xaf",
            // maximum overlong sequences
            b"\xc1\xbf",
            b"\xe0\x9f\xbf",
            b"\xf0\x8f\xbf\xbf",
            b"\xf8\x87\xbf\xbf\xbf",
            b"\xfc\x83\xbf\xbf\xbf\xbf",
            // overlong NUL
            b"\xc0\x80",
            b"\xe0\x80\x80",
            b"\xf0\x80\x80\x80",
            b"\xf8\x80\x80\x80\x80",
            b"\xfc\x80\x80\x80\x80\x80",
            // invalid continuation
            b"\xc0\x7f",
            // codepoint too high
            b"\xf5\x8f\xbf\xbf",
            b"\xf4\xbf\xbf\xbf",
            // surrogates
            b"\xed\xb8\x88",
        ];
        for s in cases {
            let (cp, len) = parse_utf8_codepoint(s);
            assert_eq!(cp, None, "{s:x?}");
            assert!(len >= 1 && len <= s.len(), "{s:x?}");
        }
    }

    #[test]
    fn verify_encoding_replaces_bad_sequences() {
        assert_eq!(verify_encoding(b"abc"), ("abc".to_owned(), true));
        assert_eq!(verify_encoding(b"a\xffb"), ("a_b".to_owned(), false));
        assert_eq!(verify_encoding(b"\xc3"), ("_".to_owned(), false));
        assert_eq!(verify_encoding(b"\xed\xb8\x88x"), ("_x".to_owned(), false));
    }
}
