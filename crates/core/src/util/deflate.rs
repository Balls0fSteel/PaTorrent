//! Deflate (RFC 1951), the counterpart of [`puff`](super::puff): LZ77 over the 32 KiB window
//! with hash chains and lazy matching (zlib's `deflate_slow` at its best level), then each run
//! of symbols as a dynamic Huffman block, or a fixed or stored one when that is smaller.
//! Written for compressing once and inflating often, such as the language tables built into
//! the binaries (D26): small output, not speed.
//!
//! This file uses `std` only, so `crates/frontend/build.rs` can include it with `#[path]`.

const WINDOW: usize = 1 << 15;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
/// How many earlier positions with the same hash a match search looks at.
const MAX_CHAIN: usize = 4096;
/// zlib's `TOO_FAR`: a three-byte match further back than this costs more than its literals.
const TOO_FAR: usize = 4096;
const HASH_BITS: u32 = 15;
const NONE: usize = usize::MAX;
/// Symbols per block; each block gets its own Huffman codes.
const BLOCK_SYMBOLS: usize = 1 << 15;

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195,
    227, 258,
];
const LEN_EXTRA: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073,
    4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
/// The order the code-length code's lengths are sent in.
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// `data` as a raw deflate stream (no zlib or gzip wrapper), ending with a final block.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let symbols = lz77(data);
    let mut w = Bits { out: Vec::with_capacity(data.len() / 3 + 16), acc: 0, n: 0 };
    let mut start = 0;
    let mut chunks = symbols.chunks(BLOCK_SYMBOLS).peekable();
    if chunks.peek().is_none() {
        write_block(&mut w, &[], &[], true);
    }
    while let Some(block) = chunks.next() {
        let bytes: usize = block.iter().map(|s| if s.dist == 0 { 1 } else { s.len as usize }).sum();
        write_block(&mut w, block, &data[start..start + bytes], chunks.peek().is_none());
        start += bytes;
    }
    w.align();
    w.out
}

/// A literal byte (`dist` 0, the byte in `len`) or a match of `len` bytes `dist` back.
#[derive(Clone, Copy)]
struct Symbol {
    len: u16,
    dist: u16,
}

fn lz77(data: &[u8]) -> Vec<Symbol> {
    let mut m = Matcher { data, head: vec![NONE; 1 << HASH_BITS], prev: vec![NONE; WINDOW] };
    let mut out = Vec::with_capacity(data.len() / 2);
    let literal = |b: u8| Symbol { len: b as u16, dist: 0 };
    // zlib's lazy evaluation: a match found at `i - 1` is held back while the match at `i` is
    // looked up, and dropped for a literal when the later one is longer.
    let (mut held, mut held_len, mut held_dist) = (false, 0, 0);
    let mut i = 0;
    while i < data.len() {
        let (len, dist) = m.longest(i);
        m.insert(i);
        if held {
            if held_len >= MIN_MATCH && len <= held_len {
                out.push(Symbol { len: held_len as u16, dist: held_dist as u16 });
                let end = i - 1 + held_len;
                for j in i + 1..end {
                    m.insert(j);
                }
                i = end;
                held = false;
                continue;
            }
            out.push(literal(data[i - 1]));
        }
        (held, held_len, held_dist) = (true, len, dist);
        i += 1;
    }
    if held {
        out.push(literal(data[data.len() - 1]));
    }
    out
}

struct Matcher<'a> {
    data: &'a [u8],
    /// The latest position of each three-byte hash.
    head: Vec<usize>,
    /// The position before it with the same hash, by position modulo the window.
    prev: Vec<usize>,
}

impl Matcher<'_> {
    fn hash(&self, i: usize) -> usize {
        let d = self.data;
        let v = u32::from_le_bytes([d[i], d[i + 1], d[i + 2], 0]);
        (v.wrapping_mul(0x9e37_79b1) >> (32 - HASH_BITS)) as usize
    }

    fn insert(&mut self, i: usize) {
        if i + MIN_MATCH <= self.data.len() {
            let h = self.hash(i);
            self.prev[i % WINDOW] = self.head[h];
            self.head[h] = i;
        }
    }

    /// The longest match for position `i` (not yet inserted) as `(length, distance)`, or a
    /// length below [`MIN_MATCH`]. Chains only go back, so an entry is never stale: a newer
    /// position in the same slot would be `i` or later.
    fn longest(&self, i: usize) -> (usize, usize) {
        let d = self.data;
        if i + MIN_MATCH > d.len() {
            return (0, 0);
        }
        let limit = MAX_MATCH.min(d.len() - i);
        let (mut best, mut best_dist) = (MIN_MATCH - 1, 0);
        let mut cand = self.head[self.hash(i)];
        let mut chain = MAX_CHAIN;
        while cand != NONE && i - cand <= WINDOW && chain > 0 {
            if d[cand + best] == d[i + best] {
                let len =
                    d[cand..cand + limit].iter().zip(&d[i..i + limit]).take_while(|(a, b)| a == b).count();
                if len > best {
                    (best, best_dist) = (len, i - cand);
                    if len == limit {
                        break;
                    }
                }
            }
            cand = self.prev[cand % WINDOW];
            chain -= 1;
        }
        if best == MIN_MATCH && best_dist > TOO_FAR { (0, 0) } else { (best, best_dist) }
    }
}

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    /// The low `bits` bits of `value`, least significant first.
    fn put(&mut self, value: u32, bits: u32) {
        self.acc |= (value as u64) << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    fn align(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
            (self.acc, self.n) = (0, 0);
        }
    }
}

fn len_code(len: u16) -> usize {
    LEN_BASE.iter().rposition(|&b| b <= len).unwrap_or(0)
}

fn dist_code(dist: u16) -> usize {
    DIST_BASE.iter().rposition(|&b| b <= dist).unwrap_or(0)
}

/// A Huffman code: each symbol's length and its code, bit-reversed for sending.
struct Code {
    lens: Vec<u8>,
    codes: Vec<u16>,
}

impl Code {
    fn from_lens(lens: Vec<u8>) -> Code {
        let mut count = [0u16; 16];
        for &l in &lens {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut next = [0u16; 16];
        for bits in 1..16 {
            next[bits] = (next[bits - 1] + count[bits - 1]) << 1;
        }
        let codes = lens
            .iter()
            .map(|&l| {
                if l == 0 {
                    return 0;
                }
                let c = next[l as usize];
                next[l as usize] += 1;
                c.reverse_bits() >> (16 - l)
            })
            .collect();
        Code { lens, codes }
    }

    fn fixed() -> (Code, Code) {
        let lit = (0..288).map(|s| match s {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        });
        (Code::from_lens(lit.collect()), Code::from_lens(vec![5; 30]))
    }

    fn put(&self, w: &mut Bits, symbol: usize) {
        w.put(self.codes[symbol] as u32, self.lens[symbol] as u32);
    }
}

/// Code lengths of at most `max_bits` for `freqs`, every used symbol given one. Shorter
/// codes go to more frequent symbols; with two or more symbols the code is complete.
fn huffman_lengths(freqs: &[u32], max_bits: usize) -> Vec<u8> {
    let mut lens = vec![0u8; freqs.len()];
    let mut syms: Vec<usize> = (0..freqs.len()).filter(|&s| freqs[s] > 0).collect();
    if syms.len() < 2 {
        if let Some(&s) = syms.first() {
            lens[s] = 1;
        }
        return lens;
    }
    syms.sort_by_key(|&s| (freqs[s], s));
    let n = syms.len();
    // The two-queue construction: leaves in order of weight, then the merged nodes, which
    // come out in order of weight too.
    let mut weight: Vec<u64> = syms.iter().map(|&s| freqs[s] as u64).collect();
    weight.resize(2 * n - 1, 0);
    let mut parent = vec![0usize; 2 * n - 1];
    let (mut leaf, mut node) = (0, n);
    for next in n..2 * n - 1 {
        let mut pick = || {
            if leaf < n && (node == next || weight[leaf] <= weight[node]) {
                leaf += 1;
                leaf - 1
            } else {
                node += 1;
                node - 1
            }
        };
        let (a, b) = (pick(), pick());
        weight[next] = weight[a] + weight[b];
        parent[a] = next;
        parent[b] = next;
    }
    let mut depth = vec![0usize; 2 * n - 1];
    for i in (0..2 * n - 2).rev() {
        depth[i] = depth[parent[i]] + 1;
    }
    // Codes per length, the too-long ones moved to `max_bits`, then miniz's fix-up until the
    // Kraft sum is exact again: each round takes one code off the longest length and splits
    // a shorter one in two.
    let mut count = vec![0u32; max_bits + 1];
    for &d in &depth[..n] {
        count[d.min(max_bits)] += 1;
    }
    let mut total: u64 = (1..=max_bits).map(|b| (count[b] as u64) << (max_bits - b)).sum();
    while total != 1 << max_bits {
        count[max_bits] -= 1;
        if let Some(b) = (1..max_bits).rev().find(|&b| count[b] > 0) {
            count[b] -= 1;
            count[b + 1] += 2;
        }
        total -= 1;
    }
    // The least frequent symbols take the longest codes.
    let mut bits = max_bits;
    for &s in &syms {
        while count[bits] == 0 {
            bits -= 1;
        }
        lens[s] = bits as u8;
        count[bits] -= 1;
    }
    lens
}

/// At least two used symbols, so the code built is complete (puff and zlib refuse an
/// incomplete code-length code, and zlib wants two distance codes).
fn two_used(freqs: &mut [u32]) {
    let mut used = freqs.iter().filter(|&&f| f > 0).count();
    for f in freqs.iter_mut().take(2) {
        if used >= 2 {
            break;
        }
        if *f == 0 {
            *f = 1;
            used += 1;
        }
    }
}

/// The code lengths as code-length symbols: `(symbol, extra bits value)`, 16 repeating the
/// previous length 3 to 6 times, 17 and 18 runs of zeros (3 to 10, 11 to 138).
fn run_lengths(lens: &[u8]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lens.len() {
        let l = lens[i];
        let run = lens[i..].iter().take_while(|&&x| x == l).count();
        let mut left = run;
        if l == 0 {
            while left >= 11 {
                let k = left.min(138);
                out.push((18, (k - 11) as u8));
                left -= k;
            }
            if left >= 3 {
                out.push((17, (left - 3) as u8));
                left = 0;
            }
        } else {
            out.push((l, 0));
            left -= 1;
            while left >= 3 {
                let k = left.min(6);
                out.push((16, (k - 3) as u8));
                left -= k;
            }
        }
        out.extend(std::iter::repeat_n((l, 0), left));
        i += run;
    }
    out
}

fn cl_extra(symbol: u8) -> u32 {
    match symbol {
        16 => 2,
        17 => 3,
        18 => 7,
        _ => 0,
    }
}

/// The bits `symbols` and the end of block take with these codes.
fn data_bits(symbols: &[Symbol], lit: &Code, dist: &Code) -> u64 {
    let mut bits = lit.lens[256] as u64;
    for s in symbols {
        bits += if s.dist == 0 {
            lit.lens[s.len as usize] as u64
        } else {
            let (lc, dc) = (len_code(s.len), dist_code(s.dist));
            (lit.lens[257 + lc] + LEN_EXTRA[lc] + dist.lens[dc] + DIST_EXTRA[dc]) as u64
        };
    }
    bits
}

fn write_symbols(w: &mut Bits, symbols: &[Symbol], lit: &Code, dist: &Code) {
    for s in symbols {
        if s.dist == 0 {
            lit.put(w, s.len as usize);
        } else {
            let (lc, dc) = (len_code(s.len), dist_code(s.dist));
            lit.put(w, 257 + lc);
            w.put((s.len - LEN_BASE[lc]) as u32, LEN_EXTRA[lc] as u32);
            dist.put(w, dc);
            w.put((s.dist - DIST_BASE[dc]) as u32, DIST_EXTRA[dc] as u32);
        }
    }
    lit.put(w, 256);
}

/// One block of `symbols`, which stand for `raw`: dynamic, fixed or stored, whichever is
/// smallest.
fn write_block(w: &mut Bits, symbols: &[Symbol], raw: &[u8], last: bool) {
    let mut lit_freq = [0u32; 286];
    let mut dist_freq = [0u32; 30];
    for s in symbols {
        if s.dist == 0 {
            lit_freq[s.len as usize] += 1;
        } else {
            lit_freq[257 + len_code(s.len)] += 1;
            dist_freq[dist_code(s.dist)] += 1;
        }
    }
    lit_freq[256] = 1;
    two_used(&mut lit_freq);
    two_used(&mut dist_freq);
    let lit = Code::from_lens(huffman_lengths(&lit_freq, 15));
    let dist = Code::from_lens(huffman_lengths(&dist_freq, 15));
    let hlit = 257.max(lit.lens.iter().rposition(|&l| l > 0).map_or(0, |p| p + 1));
    let hdist = 1.max(dist.lens.iter().rposition(|&l| l > 0).map_or(0, |p| p + 1));
    let all: Vec<u8> = lit.lens[..hlit].iter().chain(&dist.lens[..hdist]).copied().collect();
    let runs = run_lengths(&all);
    let mut cl_freq = [0u32; 19];
    for &(s, _) in &runs {
        cl_freq[s as usize] += 1;
    }
    two_used(&mut cl_freq);
    let cl = Code::from_lens(huffman_lengths(&cl_freq, 7));
    let hclen = 4.max(CL_ORDER.iter().rposition(|&s| cl.lens[s] > 0).map_or(0, |p| p + 1));

    let dynamic = 3
        + 14
        + 3 * hclen as u64
        + runs.iter().map(|&(s, _)| cl.lens[s as usize] as u64 + cl_extra(s) as u64).sum::<u64>()
        + data_bits(symbols, &lit, &dist);
    let (fixed_lit, fixed_dist) = Code::fixed();
    let fixed = 3 + data_bits(symbols, &fixed_lit, &fixed_dist);
    // Each stored block: its 3 header bits, up to 7 bits to the byte boundary, LEN and NLEN.
    let stored_blocks = raw.len().div_ceil(0xffff).max(1) as u64;
    let stored = stored_blocks * (3 + 7 + 32) + 8 * raw.len() as u64;

    if stored < dynamic.min(fixed) {
        let mut chunks = raw.chunks(0xffff).peekable();
        loop {
            let chunk = chunks.next().unwrap_or_default();
            let final_chunk = chunks.peek().is_none();
            w.put((last && final_chunk) as u32, 1);
            w.put(0, 2);
            w.align();
            let len = chunk.len() as u16;
            w.out.extend_from_slice(&len.to_le_bytes());
            w.out.extend_from_slice(&(!len).to_le_bytes());
            w.out.extend_from_slice(chunk);
            if final_chunk {
                break;
            }
        }
    } else if fixed <= dynamic {
        w.put(last as u32, 1);
        w.put(1, 2);
        write_symbols(w, symbols, &fixed_lit, &fixed_dist);
    } else {
        w.put(last as u32, 1);
        w.put(2, 2);
        w.put((hlit - 257) as u32, 5);
        w.put((hdist - 1) as u32, 5);
        w.put((hclen - 4) as u32, 4);
        for &s in &CL_ORDER[..hclen] {
            w.put(cl.lens[s] as u32, 3);
        }
        for &(s, extra) in &runs {
            cl.put(w, s as usize);
            w.put(extra as u32, cl_extra(s));
        }
        write_symbols(w, symbols, &lit, &dist);
    }
}

#[cfg(test)]
mod tests {
    use super::super::puff::inflate;
    use super::*;

    fn round_trip(data: &[u8]) -> usize {
        let packed = deflate(data);
        let (out, used) =
            inflate(&packed, data.len()).unwrap_or_else(|e| panic!("{e} ({} bytes)", data.len()));
        assert!(out == data, "round trip of {} bytes differs", data.len());
        assert_eq!(used, packed.len(), "the stream ends at its last byte");
        packed.len()
    }

    /// xorshift: deterministic noise.
    fn noise(n: usize, mut seed: u64) -> Vec<u8> {
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect()
    }

    fn text(n: usize) -> Vec<u8> {
        let words = ["torrent", "peer", "seed", "the ", "tracker", "piece", ", ", "download", "\n", "upload"];
        let mut r = noise(n, 7).into_iter();
        let mut out = Vec::new();
        while out.len() < n {
            out.extend_from_slice(words[r.next().unwrap_or(0) as usize % words.len()].as_bytes());
        }
        out.truncate(n);
        out
    }

    #[test]
    fn empty_input_is_one_fixed_block() {
        assert_eq!(deflate(b""), [0x03, 0x00]);
        round_trip(b"");
    }

    #[test]
    fn small_inputs() {
        for data in
            [&b"a"[..], b"ab", b"aaa", b"aaaa", b"hello, hello, hello world", &[0u8; 1], &[255u8; 300]]
        {
            round_trip(data);
        }
        let all: Vec<u8> = (0..=255).collect();
        round_trip(&all);
    }

    #[test]
    fn text_compresses() {
        let data = text(200_000);
        let packed = round_trip(&data);
        assert!(packed * 3 < data.len(), "{packed} of {}", data.len());
    }

    #[test]
    fn noise_is_stored() {
        let data = noise(150_000, 1);
        let packed = round_trip(&data);
        // stored blocks: five bytes of framing per block over the data
        assert!(packed <= data.len() + 5 * data.len().div_ceil(BLOCK_SYMBOLS), "{packed}");
    }

    #[test]
    fn long_runs_and_far_matches() {
        round_trip(&vec![0u8; 100_000]);
        round_trip(&vec![b'x'; 259]);
        // a block repeated exactly one window back, and just beyond it
        for gap in [WINDOW, WINDOW - 300, WINDOW + 1] {
            let block = noise(300, 9);
            let mut data = block.clone();
            data.extend(noise(gap - block.len().min(gap), 3));
            data.extend(&block);
            round_trip(&data);
        }
    }

    #[test]
    fn mixed_data_over_several_blocks() {
        let mut data = Vec::new();
        for i in 0..40 {
            data.extend(if i % 3 == 0 { noise(5000, i) } else { text(9000) });
        }
        assert!(lz77(&data).len() > BLOCK_SYMBOLS, "more than one block");
        round_trip(&data);
    }

    #[test]
    fn huffman_lengths_are_limited_and_complete() {
        // Fibonacci weights make a tree as deep as there are symbols.
        let mut fib = vec![1u32, 1];
        while fib.len() < 30 {
            let n = fib[fib.len() - 1] + fib[fib.len() - 2];
            fib.push(n);
        }
        for max in [7, 15] {
            let lens = huffman_lengths(&fib, max);
            assert!(lens.iter().all(|&l| l >= 1 && l as usize <= max), "{lens:?}");
            let kraft: u64 = lens.iter().map(|&l| 1u64 << (max - l as usize)).sum();
            assert_eq!(kraft, 1 << max);
            // more frequent, never longer
            assert!(lens.windows(2).all(|w| w[0] >= w[1]), "{lens:?}");
        }
        assert_eq!(huffman_lengths(&[0, 5, 0], 15), [0, 1, 0]);
        assert_eq!(huffman_lengths(&[3, 3], 15), [1, 1]);
    }

    #[test]
    fn run_length_coding_of_code_lengths() {
        let lens = [3u8, 3, 3, 3, 3, 3, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0];
        let runs = run_lengths(&lens);
        assert_eq!(runs, [(3, 0), (16, 3), (3, 0), (18, 1), (5, 0), (0, 0), (0, 0)]);
        let mut back = Vec::new();
        for (s, extra) in runs {
            match s {
                16 => back.extend(std::iter::repeat_n(*back.last().unwrap(), 3 + extra as usize)),
                17 => back.extend(std::iter::repeat_n(0, 3 + extra as usize)),
                18 => back.extend(std::iter::repeat_n(0, 11 + extra as usize)),
                l => back.push(l),
            }
        }
        assert_eq!(back, lens);
    }
}
