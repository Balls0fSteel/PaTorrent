//! Merkle–Damgård block buffering shared by SHA-1 and SHA-256 (64-byte blocks, big-endian 64-bit
//! bit length in the final block).

#[derive(Clone)]
pub(crate) struct BlockBuffer {
    buf: [u8; 64],
    len: usize,
    /// Total bytes hashed so far.
    total: u64,
}

impl BlockBuffer {
    pub(crate) const fn new() -> BlockBuffer {
        BlockBuffer { buf: [0; 64], len: 0, total: 0 }
    }

    /// Feeds `data`; `compress` is called with runs of whole 64-byte blocks.
    pub(crate) fn update(&mut self, mut data: &[u8], mut compress: impl FnMut(&[u8])) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.len > 0 {
            let n = (64 - self.len).min(data.len());
            self.buf[self.len..self.len + n].copy_from_slice(&data[..n]);
            self.len += n;
            data = &data[n..];
            if self.len < 64 {
                return;
            }
            compress(&self.buf);
            self.len = 0;
        }
        let whole = data.len() - data.len() % 64;
        if whole > 0 {
            compress(&data[..whole]);
        }
        let rest = &data[whole..];
        self.buf[..rest.len()].copy_from_slice(rest);
        self.len = rest.len();
    }

    /// Appends the padding and length, compressing the final block(s).
    pub(crate) fn finish(&mut self, mut compress: impl FnMut(&[u8])) {
        let bits = self.total.wrapping_mul(8);
        self.buf[self.len] = 0x80;
        self.len += 1;
        if self.len > 56 {
            self.buf[self.len..].fill(0);
            compress(&self.buf);
            self.len = 0;
        }
        self.buf[self.len..56].fill(0);
        self.buf[56..].copy_from_slice(&bits.to_be_bytes());
        compress(&self.buf);
        self.len = 0;
    }
}
