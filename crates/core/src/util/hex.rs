//! Hex encoding (libtorrent `aux::to_hex` / `aux::from_hex`).

const DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Lower-case hex encoding.
pub fn encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0xf) as usize] as char);
    }
    s
}

/// Decodes `s` into exactly `out.len()` bytes. Accepts either case; returns `false` on any
/// length mismatch or non-hex character.
pub fn decode_into(s: &str, out: &mut [u8]) -> bool {
    let s = s.as_bytes();
    if s.len() != out.len() * 2 {
        return false;
    }
    for (o, pair) in out.iter_mut().zip(s.chunks_exact(2)) {
        match (nibble(pair[0]), nibble(pair[1])) {
            (Some(hi), Some(lo)) => *o = (hi << 4) | lo,
            _ => return false,
        }
    }
    true
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_case() {
        assert_eq!(encode(&[0x00, 0x7f, 0xff]), "007fff");
        let mut b = [0u8; 3];
        assert!(decode_into("007FfF", &mut b));
        assert_eq!(b, [0x00, 0x7f, 0xff]);
        assert!(!decode_into("007ff", &mut b));
        assert!(!decode_into("007fgf", &mut b));
    }
}
