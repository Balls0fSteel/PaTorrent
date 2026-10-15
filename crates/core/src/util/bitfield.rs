//! Fixed-length bit set used for piece maps (libtorrent `bitfield` / `typed_bitfield`).
//!
//! Minimal Phase 0 version; Phase 1 ports the rest of `bitfield.cpp` (popcount fast paths,
//! `find_first_set`, wire-format helpers).

/// Bit `i` lives in byte `i / 8` at mask `0x80 >> (i % 8)`, the BitTorrent wire order, so the
/// bytes can be sent as a `bitfield` message without conversion.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Bitfield {
    bytes: Vec<u8>,
    len: usize,
}

impl Bitfield {
    pub fn new(len: usize, value: bool) -> Bitfield {
        let mut bf = Bitfield { bytes: vec![if value { 0xff } else { 0 }; len.div_ceil(8)], len };
        bf.clear_trailing_bits();
        bf
    }

    /// Builds a bitfield from wire-format bytes. Returns `None` if `bytes` is not exactly
    /// `ceil(len / 8)` long. Spare trailing bits are cleared (libtorrent ignores them too).
    pub fn from_bytes(bytes: &[u8], len: usize) -> Option<Bitfield> {
        if bytes.len() != len.div_ceil(8) {
            return None;
        }
        let mut bf = Bitfield { bytes: bytes.to_vec(), len };
        bf.clear_trailing_bits();
        Some(bf)
    }

    pub fn set_all(&mut self, value: bool) {
        self.bytes.fill(if value { 0xff } else { 0 });
        self.clear_trailing_bits();
    }

    /// True if every bit set in `self` is also set in `other`.
    pub fn is_subset_of(&self, other: &Bitfield) -> bool {
        self.len == other.len && self.bytes.iter().zip(&other.bytes).all(|(a, b)| a & !b == 0)
    }

    pub fn none_set(&self) -> bool {
        self.bytes.iter().all(|&b| b == 0)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, i: usize) -> bool {
        assert!(i < self.len, "bit {i} out of range {}", self.len);
        self.bytes[i / 8] & (0x80 >> (i % 8)) != 0
    }

    pub fn set(&mut self, i: usize, value: bool) {
        assert!(i < self.len, "bit {i} out of range {}", self.len);
        let mask = 0x80 >> (i % 8);
        if value { self.bytes[i / 8] |= mask } else { self.bytes[i / 8] &= !mask }
    }

    pub fn count(&self) -> usize {
        self.bytes.iter().map(|b| b.count_ones() as usize).sum()
    }

    pub fn all_set(&self) -> bool {
        self.count() == self.len
    }

    /// Wire-format bytes (trailing pad bits are always zero).
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn iter(&self) -> impl Iterator<Item = bool> + '_ {
        (0..self.len).map(|i| self.get(i))
    }

    fn clear_trailing_bits(&mut self) {
        if self.len % 8 != 0 {
            let last = self.bytes.len() - 1;
            self.bytes[last] &= 0xff << (8 - self.len % 8);
        }
    }
}

impl std::fmt::Debug for Bitfield {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bitfield({}/{} set)", self.count(), self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_order_and_padding() {
        let mut bf = Bitfield::new(10, false);
        bf.set(0, true);
        bf.set(9, true);
        assert_eq!(bf.as_bytes(), &[0x80, 0x40]);
        assert_eq!(bf.count(), 2);
        let full = Bitfield::new(10, true);
        assert_eq!(full.as_bytes(), &[0xff, 0xc0]);
        assert!(full.all_set());
        assert!(Bitfield::new(0, true).all_set());
    }

    #[test]
    fn from_bytes_and_subsets() {
        assert!(Bitfield::from_bytes(&[0xff], 9).is_none());
        let bf = Bitfield::from_bytes(&[0xff, 0xff], 9).unwrap();
        assert_eq!(bf.as_bytes(), &[0xff, 0x80], "spare bits are cleared");
        let mut half = Bitfield::new(9, false);
        assert!(half.none_set() && half.is_subset_of(&bf));
        half.set(3, true);
        assert!(half.is_subset_of(&bf) && !bf.is_subset_of(&half));
        half.set_all(true);
        assert_eq!(half, bf);
    }
}
