// Ported from libtorrent-rasterbar 2.0.11: src/puff.cpp, include/libtorrent/puff.hpp
// puff.c, Copyright (C) 2002-2013 Mark Adler, version 2.3. zlib-style license, see
// THIRD-PARTY-NOTICES.md. This is an altered version of the original software.
// Changes: running out of input is an `Err` carried up with `?` instead of `longjmp`; the
// output is a `Vec` that grows up to a caller's limit instead of a fixed buffer (no scanning
// mode); the fixed Huffman tables are built once behind a `OnceLock`; errors are an enum.

//! Inflate (RFC 1951) after Mark Adler's `puff`: small and simple rather than fast, for the
//! occasional compressed block list or HTTP body.

use std::sync::OnceLock;

const MAXBITS: usize = 15;
const MAXLCODES: usize = 286;
const MAXDCODES: usize = 30;
const MAXCODES: usize = MAXLCODES + MAXDCODES;
const FIXLCODES: usize = 288;

/// Why inflating failed (puff's return codes 2 to -11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InflateError {
    /// 2: the input ended before the last block did.
    Incomplete,
    /// 1: the output would pass the caller's limit.
    TooLarge,
    /// -1
    InvalidBlockType,
    /// -2
    StoredLengthMismatch,
    /// -3
    TooManyCodes,
    /// -4
    CodeLengthsIncomplete,
    /// -5
    RepeatWithoutFirstLength,
    /// -6
    RepeatTooMany,
    /// -7
    InvalidLiteralLengths,
    /// -8
    InvalidDistanceLengths,
    /// -9
    MissingEndOfBlock,
    /// -10
    InvalidCode,
    /// -11
    DistanceTooFar,
}

impl std::fmt::Display for InflateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            InflateError::Incomplete => "available inflate data did not terminate",
            InflateError::TooLarge => "inflated data too large",
            InflateError::InvalidBlockType => "invalid block type (type == 3)",
            InflateError::StoredLengthMismatch => "stored block length did not match one's complement",
            InflateError::TooManyCodes => "dynamic block code description: too many length or distance codes",
            InflateError::CodeLengthsIncomplete => {
                "dynamic block code description: code lengths codes incomplete"
            }
            InflateError::RepeatWithoutFirstLength => {
                "dynamic block code description: repeat lengths with no first length"
            }
            InflateError::RepeatTooMany => {
                "dynamic block code description: repeat more than specified lengths"
            }
            InflateError::InvalidLiteralLengths => {
                "dynamic block code description: invalid literal/length code lengths"
            }
            InflateError::InvalidDistanceLengths => {
                "dynamic block code description: invalid distance code lengths"
            }
            InflateError::MissingEndOfBlock => "dynamic block code description: missing end-of-block code",
            InflateError::InvalidCode => "invalid literal/length or distance code in fixed or dynamic block",
            InflateError::DistanceTooFar => "distance is too far back in fixed or dynamic block",
        })
    }
}

impl std::error::Error for InflateError {}

type Result<T> = std::result::Result<T, InflateError>;

/// Inflates the raw deflate stream at the start of `input` into at most `max_out` bytes.
/// Returns the data and how many input bytes the stream took (the rest is whatever follows
/// it, such as a gzip trailer).
pub fn inflate(input: &[u8], max_out: usize) -> Result<(Vec<u8>, usize)> {
    let mut s = State { out: Vec::new(), max: max_out, input, incnt: 0, bitbuf: 0, bitcnt: 0 };
    loop {
        let last = s.bits(1)?;
        match s.bits(2)? {
            0 => s.stored()?,
            1 => {
                let (lencode, distcode) = fixed_tables();
                s.codes(lencode, distcode)?
            }
            2 => s.dynamic()?,
            _ => return Err(InflateError::InvalidBlockType),
        }
        if last == 1 {
            return Ok((s.out, s.incnt));
        }
    }
}

struct State<'a> {
    out: Vec<u8>,
    max: usize,
    input: &'a [u8],
    incnt: usize,
    bitbuf: u32,
    bitcnt: u32,
}

/// Huffman decoding tables: `count[len]` codes of each length, `symbol` in canonical order.
struct Huffman {
    count: [u16; MAXBITS + 1],
    symbol: [u16; FIXLCODES],
}

impl Huffman {
    fn new() -> Huffman {
        Huffman { count: [0; MAXBITS + 1], symbol: [0; FIXLCODES] }
    }
}

impl State<'_> {
    /// `need` bits from the input, least significant first; leaves under eight in the buffer.
    fn bits(&mut self, need: u32) -> Result<u32> {
        let mut val = self.bitbuf as u64;
        while self.bitcnt < need {
            let b = *self.input.get(self.incnt).ok_or(InflateError::Incomplete)?;
            self.incnt += 1;
            val |= (b as u64) << self.bitcnt;
            self.bitcnt += 8;
        }
        self.bitbuf = (val >> need) as u32;
        self.bitcnt -= need;
        Ok((val & ((1u64 << need) - 1)) as u32)
    }

    /// A stored block: byte aligned, its length and the length's complement, then the bytes.
    fn stored(&mut self) -> Result<()> {
        self.bitbuf = 0;
        self.bitcnt = 0;
        let rest = &self.input[self.incnt..];
        if rest.len() < 4 {
            return Err(InflateError::Incomplete);
        }
        let len = rest[0] as usize | (rest[1] as usize) << 8;
        if rest[2] != !len as u8 || rest[3] != (!len >> 8) as u8 {
            return Err(InflateError::StoredLengthMismatch);
        }
        let data = rest.get(4..4 + len).ok_or(InflateError::Incomplete)?;
        if self.out.len() + len > self.max {
            return Err(InflateError::TooLarge);
        }
        self.out.extend_from_slice(data);
        self.incnt += 4 + len;
        Ok(())
    }

    /// The faster of puff's two `decode`s: one code, read bit by bit, through `h`.
    fn decode(&mut self, h: &Huffman) -> Result<u16> {
        let mut bitbuf = self.bitbuf;
        let mut left = self.bitcnt as i32;
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        let mut len = 1usize;
        loop {
            while left > 0 {
                left -= 1;
                code |= (bitbuf & 1) as i32;
                bitbuf >>= 1;
                let count = h.count[len] as i32;
                if code - count < first {
                    self.bitbuf = bitbuf;
                    self.bitcnt = ((self.bitcnt as i32 - len as i32) & 7) as u32;
                    return Ok(h.symbol[(index + (code - first)) as usize]);
                }
                index += count;
                first += count;
                first <<= 1;
                code <<= 1;
                len += 1;
            }
            left = (MAXBITS + 1) as i32 - len as i32;
            if left == 0 {
                return Err(InflateError::InvalidCode);
            }
            bitbuf = *self.input.get(self.incnt).ok_or(InflateError::Incomplete)? as u32;
            self.incnt += 1;
            left = left.min(8);
        }
    }

    /// Literals and length / distance pairs up to the end-of-block code.
    fn codes(&mut self, lencode: &Huffman, distcode: &Huffman) -> Result<()> {
        const LENS: [u16; 29] = [
            3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
            163, 195, 227, 258,
        ];
        const LEXT: [u32; 29] =
            [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
        const DISTS: [u16; 30] = [
            1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049,
            3073, 4097, 6145, 8193, 12289, 16385, 24577,
        ];
        const DEXT: [u32; 30] = [
            0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
        ];
        loop {
            let symbol = self.decode(lencode)? as usize;
            match symbol {
                0..=255 => {
                    if self.out.len() == self.max {
                        return Err(InflateError::TooLarge);
                    }
                    self.out.push(symbol as u8);
                }
                256 => return Ok(()),
                _ => {
                    let symbol = symbol - 257;
                    if symbol >= 29 {
                        return Err(InflateError::InvalidCode);
                    }
                    let len = (LENS[symbol] as u32 + self.bits(LEXT[symbol])?) as usize;
                    let symbol = self.decode(distcode)? as usize;
                    // the fixed distance code has two symbols past the table (30, 31)
                    if symbol >= 30 {
                        return Err(InflateError::InvalidCode);
                    }
                    let dist = (DISTS[symbol] as u32 + self.bits(DEXT[symbol])?) as usize;
                    if dist > self.out.len() {
                        return Err(InflateError::DistanceTooFar);
                    }
                    if self.out.len() + len > self.max {
                        return Err(InflateError::TooLarge);
                    }
                    // overlapping copies repeat the last `dist` bytes, so byte by byte
                    for _ in 0..len {
                        let b = self.out[self.out.len() - dist];
                        self.out.push(b);
                    }
                }
            }
        }
    }

    /// A dynamic block: the code descriptions, then the data.
    fn dynamic(&mut self) -> Result<()> {
        const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
        let nlen = self.bits(5)? as usize + 257;
        let ndist = self.bits(5)? as usize + 1;
        let ncode = self.bits(4)? as usize + 4;
        if nlen > MAXLCODES || ndist > MAXDCODES {
            return Err(InflateError::TooManyCodes);
        }
        let mut lengths = [0u16; MAXCODES];
        for &o in &ORDER[..ncode] {
            lengths[o] = self.bits(3)? as u16;
        }
        let mut lencode = Huffman::new();
        let mut distcode = Huffman::new();
        if construct(&mut lencode, &lengths[..19]) != 0 {
            return Err(InflateError::CodeLengthsIncomplete);
        }
        let mut index = 0;
        while index < nlen + ndist {
            let symbol = self.decode(&lencode)?;
            if symbol < 16 {
                lengths[index] = symbol;
                index += 1;
                continue;
            }
            let (len, repeat) = match symbol {
                16 => {
                    if index == 0 {
                        return Err(InflateError::RepeatWithoutFirstLength);
                    }
                    (lengths[index - 1], 3 + self.bits(2)? as usize)
                }
                17 => (0, 3 + self.bits(3)? as usize),
                _ => (0, 11 + self.bits(7)? as usize),
            };
            if index + repeat > nlen + ndist {
                return Err(InflateError::RepeatTooMany);
            }
            lengths[index..index + repeat].fill(len);
            index += repeat;
        }
        if lengths[256] == 0 {
            return Err(InflateError::MissingEndOfBlock);
        }
        // incomplete codes are allowed only for a single code of length one
        let err = construct(&mut lencode, &lengths[..nlen]);
        if err != 0 && (err < 0 || nlen != (lencode.count[0] + lencode.count[1]) as usize) {
            return Err(InflateError::InvalidLiteralLengths);
        }
        let err = construct(&mut distcode, &lengths[nlen..nlen + ndist]);
        if err != 0 && (err < 0 || ndist != (distcode.count[0] + distcode.count[1]) as usize) {
            return Err(InflateError::InvalidDistanceLengths);
        }
        self.codes(&lencode, &distcode)
    }
}

/// Builds `h` from code lengths (each 0..=15). Zero for a complete code, negative for an
/// over-subscribed one (unusable), positive for an incomplete one.
fn construct(h: &mut Huffman, length: &[u16]) -> i32 {
    h.count = [0; MAXBITS + 1];
    for &l in length {
        h.count[l as usize] += 1;
    }
    if h.count[0] as usize == length.len() {
        // no codes: complete, but decode() will fail
        return 0;
    }
    let mut left = 1i32;
    for len in 1..=MAXBITS {
        left <<= 1;
        left -= h.count[len] as i32;
        if left < 0 {
            return left;
        }
    }
    let mut offs = [0u16; MAXBITS + 1];
    for len in 1..MAXBITS {
        offs[len + 1] = offs[len] + h.count[len];
    }
    for (symbol, &l) in length.iter().enumerate() {
        if l != 0 {
            h.symbol[offs[l as usize] as usize] = symbol as u16;
            offs[l as usize] += 1;
        }
    }
    left
}

/// The fixed literal / length and distance codes of block type 1.
fn fixed_tables() -> (&'static Huffman, &'static Huffman) {
    static TABLES: OnceLock<(Huffman, Huffman)> = OnceLock::new();
    let (l, d) = TABLES.get_or_init(|| {
        let mut lengths = [0u16; FIXLCODES];
        lengths[..144].fill(8);
        lengths[144..256].fill(9);
        lengths[256..280].fill(7);
        lengths[280..].fill(8);
        let mut lencode = Huffman::new();
        construct(&mut lencode, &lengths);
        let mut distcode = Huffman::new();
        construct(&mut distcode, &[5; MAXDCODES]);
        (lencode, distcode)
    });
    (l, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &[u8] = b"hello, hello, hello world";

    fn text() -> Vec<u8> {
        (0..200)
            .flat_map(|i| format!("line {i}: the quick brown fox jumps over the lazy dog\n").into_bytes())
            .collect()
    }

    /// [`text`] deflated by zlib at level 9 (one dynamic block).
    #[rustfmt::skip]
    const DYNAMIC: [u8; 558] = [157, 218, 203, 113, 19, 65, 0, 69, 209, 61, 81, 76, 8, 243, 250, 223, 100, 195, 71, 128, 65, 88, 96, 99, 192, 142, 158, 130, 12, 56, 107, 213, 91, 233, 148, 52, 115, 187, 175, 119, 247, 151, 227, 124, 125, 252, 248, 116, 57, 190, 63, 221, 189, 251, 114, 188, 125, 184, 253, 186, 63, 62, 220, 126, 31, 159, 159, 190, 126, 123, 60, 110, 63, 47, 15, 255, 62, 190, 190, 121, 121, 62, 222, 223, 62, 190, 186, 254, 221, 4, 54, 5, 54, 21, 54, 13, 54, 29, 54, 3, 54, 19, 54, 11, 54, 91, 190, 83, 130, 32, 18, 34, 20, 34, 22, 34, 24, 34, 26, 34, 28, 34, 30, 34, 32, 34, 34, 138, 136, 40, 244, 219, 32, 34, 138, 136, 40, 34, 162, 136, 136, 34, 34, 138, 136, 40, 34, 162, 136, 136, 42, 34, 170, 136, 168, 244, 119, 33, 34, 170, 136, 168, 34, 162, 138, 136, 42, 34, 170, 136, 168, 34, 162, 137, 136, 38, 34, 154, 136, 104, 244, 4, 33, 34, 154, 136, 104, 34, 162, 137, 136, 38, 34, 154, 136, 232, 34, 162, 139, 136, 46, 34, 186, 136, 232, 244, 80, 41, 34, 186, 136, 232, 34, 162, 139, 136, 46, 34, 134, 136, 24, 34, 98, 136, 136, 33, 34, 134, 136, 24, 244, 158, 33, 34, 134, 136, 24, 34, 98, 136, 136, 41, 34, 166, 136, 152, 34, 98, 138, 136, 41, 34, 166, 136, 152, 244, 234, 41, 34, 166, 136, 152, 34, 98, 137, 136, 37, 34, 150, 136, 88, 34, 98, 137, 136, 37, 34, 150, 136, 88, 84, 35, 68, 196, 18, 17, 91, 68, 108, 17, 177, 69, 196, 22, 17, 91, 68, 108, 17, 177, 69, 196, 22, 17, 155, 2, 149, 21, 42, 74, 84, 39, 53, 170, 147, 34, 213, 73, 149, 234, 164, 76, 117, 82, 167, 58, 41, 84, 157, 84, 170, 78, 74, 85, 39, 217, 192, 124, 73, 54, 44, 96, 90, 193, 180, 132, 105, 13, 211, 34, 166, 85, 76, 203, 152, 212, 49, 67, 33, 51, 197, 218, 54, 217, 160, 150, 25, 138, 153, 161, 154, 25, 202, 153, 161, 158, 25, 10, 154, 161, 162, 25, 74, 154, 161, 166, 153, 106, 7, 31, 100, 131, 178, 102, 168, 107, 134, 194, 102, 168, 108, 134, 210, 102, 168, 109, 134, 226, 102, 168, 110, 134, 242, 102, 154, 157, 138, 145, 13, 42, 156, 161, 196, 25, 106, 156, 161, 200, 25, 170, 156, 161, 204, 25, 234, 156, 161, 208, 25, 42, 157, 233, 118, 100, 74, 54, 40, 118, 134, 106, 103, 40, 119, 134, 122, 103, 40, 120, 134, 138, 103, 40, 121, 134, 154, 103, 40, 122, 102, 216, 121, 58, 217, 160, 238, 25, 10, 159, 161, 242, 25, 74, 159, 161, 246, 25, 138, 159, 161, 250, 25, 202, 159, 161, 254, 153, 105, 151, 45, 200, 6, 37, 208, 80, 3, 13, 69, 208, 80, 5, 13, 101, 208, 80, 7, 13, 133, 208, 80, 9, 13, 165, 208, 44, 187, 137, 67, 54, 168, 134, 134, 114, 104, 168, 135, 134, 130, 104, 168, 136, 134, 146, 104, 168, 137, 134, 162, 104, 168, 138, 102, 219, 53, 173, 255, 180, 241, 7];

    #[test]
    fn stored_fixed_and_dynamic_blocks() {
        // zlib level 0: one stored block
        let stored = [
            1, 25, 0, 230, 255, 104, 101, 108, 108, 111, 44, 32, 104, 101, 108, 108, 111, 44, 32, 104, 101,
            108, 108, 111, 32, 119, 111, 114, 108, 100,
        ];
        assert_eq!(inflate(&stored, 1000).unwrap(), (HELLO.to_vec(), stored.len()));
        // Z_FIXED: one fixed-code block with a length / distance pair
        let fixed = [203, 72, 205, 201, 201, 215, 81, 200, 64, 162, 20, 202, 243, 139, 114, 82, 0];
        assert_eq!(inflate(&fixed, 1000).unwrap(), (HELLO.to_vec(), fixed.len()));
        let (out, used) = inflate(&DYNAMIC, 1 << 20).unwrap();
        assert_eq!(used, DYNAMIC.len());
        assert_eq!(out, text());
        // bytes after the stream are left alone
        let mut more = fixed.to_vec();
        more.extend_from_slice(b"trailer");
        assert_eq!(inflate(&more, 1000).unwrap().1, fixed.len());
    }

    #[test]
    fn errors() {
        let fixed = [203, 72, 205, 201, 201, 215, 81, 200, 64, 162, 20, 202, 243, 139, 114, 82, 0];
        // every prefix of a stream is incomplete, never a panic
        for n in 0..fixed.len() {
            assert_eq!(inflate(&fixed[..n], 1000), Err(InflateError::Incomplete), "{n}");
        }
        for n in 0..DYNAMIC.len() {
            assert!(inflate(&DYNAMIC[..n], 1 << 20).is_err(), "{n}");
        }
        assert_eq!(inflate(&fixed, HELLO.len() - 1), Err(InflateError::TooLarge));
        assert_eq!(inflate(&DYNAMIC, 1000), Err(InflateError::TooLarge));
        // block type 3
        assert_eq!(inflate(&[0b111], 10), Err(InflateError::InvalidBlockType));
        // a stored block whose length's complement is wrong
        assert_eq!(inflate(&[1, 1, 0, 0, 0, b'x'], 10), Err(InflateError::StoredLengthMismatch));
        // a fixed block starting with a length / distance pair: nothing to copy from yet
        // (BFINAL 1, type 01, then length symbol 257 = 0000001 and distance 0 = 00000)
        assert_eq!(inflate(&[0b0000_0011, 0b0000_0010, 0], 10), Err(InflateError::DistanceTooFar));
        // flipping each bit of the dynamic stream gives an error or other data, never a panic
        for i in 0..DYNAMIC.len() * 8 {
            let mut d = DYNAMIC;
            d[i / 8] ^= 1 << (i % 8);
            let _ = inflate(&d, 1 << 20);
        }
    }
}
