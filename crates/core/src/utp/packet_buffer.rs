// Ported from libtorrent-rasterbar 2.0.11: src/packet_buffer.cpp, include/libtorrent/aux_/packet_buffer.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: slots hold `Option<Box<Packet>>` in a Vec instead of pooled `packet_ptr`s; otherwise
// the same power-of-two circular buffer over 16-bit sequence numbers.

//! A circular buffer indexed by (wrapping) 16-bit sequence numbers: the packets we sent and
//! wait to have acknowledged, and the ones we received out of order.

use super::Packet;
use super::compare_less_wrap;

#[derive(Default)]
pub(crate) struct PacketBuffer {
    storage: Vec<Option<Box<Packet>>>,
    /// Number of occupied slots.
    size: usize,
    /// Lowest index with an element (also decides what each slot refers to).
    first: u32,
    /// One past the highest used index.
    last: u32,
}

impl PacketBuffer {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    pub fn capacity(&self) -> u32 {
        self.storage.len() as u32
    }

    /// The lowest index holding an element.
    #[cfg(test)]
    pub fn cursor(&self) -> u32 {
        self.first
    }

    /// Indices from the cursor up to the highest used one.
    pub fn span(&self) -> u32 {
        self.last.wrapping_sub(self.first) & 0xffff
    }

    fn mask(&self) -> u32 {
        self.capacity().wrapping_sub(1)
    }

    /// Stores `value` at `idx`, returning what was there before.
    pub fn insert(&mut self, idx: u32, value: Box<Packet>) -> Option<Box<Packet>> {
        debug_assert!(idx <= 0xffff);
        if self.size != 0 {
            if compare_less_wrap(idx, self.first, 0xffff) {
                // Index comes before `first`. If we have room, we can simply adjust `first`
                // backward.
                let mut free_space = 0u32;
                let cap = self.capacity();
                let mut i = self.first.wrapping_sub(1) & self.mask();
                while i != (self.first & self.mask()) {
                    if self.storage[(i & self.mask()) as usize].is_some() {
                        break;
                    }
                    free_space += 1;
                    i = i.wrapping_sub(1) & self.mask();
                }
                let distance = self.first.wrapping_sub(idx) & 0xffff;
                if distance > free_space {
                    self.reserve(distance + cap - free_space);
                }
                self.first = idx;
            } else if idx >= self.first + self.capacity() {
                self.reserve(idx - self.first + 1);
            } else if idx < self.first {
                // We have wrapped.
                let end = (self.first + self.capacity()) & 0xffff;
                if idx >= end && self.capacity() < 0xffff {
                    self.reserve(self.capacity() + (idx + 1 - end));
                }
            }
            if compare_less_wrap(self.last, (idx + 1) & 0xffff, 0xffff) {
                self.last = (idx + 1) & 0xffff;
            }
        } else {
            self.first = idx;
            self.last = (idx + 1) & 0xffff;
        }
        if self.capacity() == 0 {
            self.reserve(16);
        }
        let slot = (idx & self.mask()) as usize;
        let old = self.storage[slot].replace(value);
        if self.size == 0 {
            self.first = idx;
        }
        // replacing an old value does not grow the buffer
        if old.is_none() {
            self.size += 1;
        }
        old
    }

    pub fn at(&self, idx: u32) -> Option<&Packet> {
        if idx >= self.first + self.capacity() || compare_less_wrap(idx, self.first, 0xffff) {
            return None;
        }
        self.storage[(idx & self.mask()) as usize].as_deref()
    }

    pub fn at_mut(&mut self, idx: u32) -> Option<&mut Packet> {
        if idx >= self.first + self.capacity() || compare_less_wrap(idx, self.first, 0xffff) {
            return None;
        }
        let mask = self.mask();
        self.storage[(idx & mask) as usize].as_deref_mut()
    }

    fn reserve(&mut self, size: u32) {
        debug_assert!(size <= 0xffff);
        let mut new_size = if self.capacity() == 0 { 16 } else { self.capacity() };
        while new_size < size {
            new_size <<= 1;
        }
        let mut new_storage: Vec<Option<Box<Packet>>> = (0..new_size).map(|_| None).collect();
        let cap = self.capacity();
        for i in self.first..self.first + cap {
            new_storage[(i & (new_size - 1)) as usize] = self.storage[(i & (cap - 1)) as usize].take();
        }
        self.storage = new_storage;
    }

    pub fn remove(&mut self, idx: u32) -> Option<Box<Packet>> {
        if idx >= self.first + self.capacity() || compare_less_wrap(idx, self.first, 0xffff) {
            return None;
        }
        let mask = self.mask();
        let old = self.storage[(idx & mask) as usize].take();
        if old.is_some() {
            self.size -= 1;
            if self.size == 0 {
                self.last = self.first;
            }
        }
        if idx == self.first && self.size != 0 {
            self.first += 1;
            for _ in 0..self.capacity() {
                if self.storage[(self.first & mask) as usize].is_some() {
                    break;
                }
                self.first += 1;
            }
            self.first &= 0xffff;
        }
        if ((idx + 1) & 0xffff) == self.last && self.size != 0 {
            self.last = self.last.wrapping_sub(1);
            for _ in 0..self.capacity() {
                if self.storage[(self.last & mask) as usize].is_some() {
                    break;
                }
                self.last = self.last.wrapping_sub(1);
            }
            self.last = self.last.wrapping_add(1) & 0xffff;
        }
        old
    }
}

// libtorrent test/test_packet_buffer.cpp
#[cfg(test)]
mod tests {
    use super::*;

    fn make_pkt(val: u8) -> Box<Packet> {
        let mut p = Packet::new(20);
        p.buf[0] = val;
        Box::new(p)
    }

    fn get_val(p: Option<&Packet>) -> u8 {
        p.expect("packet present").buf[0]
    }

    #[test]
    fn insert() {
        let mut pb = PacketBuffer::default();
        assert_eq!(pb.capacity(), 0);
        assert_eq!(pb.len(), 0);
        assert_eq!(pb.span(), 0);

        pb.insert(123, make_pkt(123));
        assert!(pb.at(123 + 16).is_none());
        assert_eq!(get_val(pb.at(123)), 123);
        assert!(pb.capacity() > 0);
        assert_eq!(pb.len(), 1);
        assert_eq!(pb.span(), 1);
        assert_eq!(pb.cursor(), 123);

        pb.insert(125, make_pkt(125));
        assert_eq!(get_val(pb.at(125)), 125);
        assert_eq!(pb.len(), 2);
        assert_eq!(pb.span(), 3);
        assert_eq!(pb.cursor(), 123);

        pb.insert(500, make_pkt(4));
        assert_eq!(pb.len(), 3);
        assert_eq!(pb.span(), 501 - 123);
        assert_eq!(pb.capacity(), 512);

        pb.insert(500, make_pkt(5));
        assert_eq!(pb.len(), 3);
        assert_eq!(get_val(pb.insert(500, make_pkt(4)).as_deref()), 5);
        assert_eq!(pb.len(), 3);

        assert_eq!(get_val(pb.remove(123).as_deref()), 123);
        assert_eq!(pb.len(), 2);
        assert_eq!(pb.span(), 501 - 125);
        assert_eq!(pb.cursor(), 125);
        assert_eq!(get_val(pb.remove(125).as_deref()), 125);
        assert_eq!(pb.len(), 1);
        assert_eq!(pb.span(), 1);
        assert_eq!(pb.cursor(), 500);

        assert_eq!(get_val(pb.remove(500).as_deref()), 4);
        assert_eq!(pb.len(), 0);
        assert_eq!(pb.span(), 0);

        for i in 0..0xff {
            let mut index = (i + 0xfff0) & 0xffff;
            pb.insert(index, make_pkt((index + 1) as u8));
            assert_eq!(pb.capacity(), 512);
            if i >= 14 {
                index = (index.wrapping_sub(14)) & 0xffff;
                assert_eq!(get_val(pb.remove(index).as_deref()), (index + 1) as u8);
                assert_eq!(pb.len(), 14);
            }
        }
    }

    #[test]
    fn wrap() {
        let mut pb = PacketBuffer::default();
        assert_eq!(pb.len(), 0);
        pb.insert(0xfffe, make_pkt(1));
        assert_eq!(get_val(pb.at(0xfffe)), 1);
        pb.insert(2, make_pkt(2));
        assert_eq!(get_val(pb.at(2)), 2);
        pb.remove(0xfffe);
        assert!(pb.at(0xfffe).is_none());
        assert_eq!(get_val(pb.at(2)), 2);
    }

    #[test]
    fn wrap2() {
        let mut pb = PacketBuffer::default();
        assert_eq!(pb.len(), 0);
        pb.insert(0xfff3, make_pkt(1));
        assert_eq!(get_val(pb.at(0xfff3)), 1);
        let new_index = (0xfff3 + pb.capacity()) & 0xffff;
        pb.insert(new_index, make_pkt(2));
        assert_eq!(get_val(pb.at(new_index)), 2);
        let old = pb.remove(0xfff3);
        assert_eq!(get_val(old.as_deref()), 1);
        assert!(pb.at(0xfff3).is_none());
        assert_eq!(get_val(pb.at(new_index)), 2);
    }

    #[test]
    fn reverse_wrap() {
        let mut pb = PacketBuffer::default();
        assert_eq!(pb.len(), 0);
        pb.insert(0xfff3, make_pkt(1));
        assert_eq!(get_val(pb.at(0xfff3)), 1);
        let new_index = (0xfff3 + pb.capacity()) & 0xffff;
        pb.insert(new_index, make_pkt(2));
        assert_eq!(get_val(pb.at(new_index)), 2);
        let old = pb.remove(0xfff3);
        assert_eq!(get_val(old.as_deref()), 1);
        assert!(pb.at(0xfff3).is_none());
        assert_eq!(get_val(pb.at(new_index)), 2);
        pb.insert(0xffff, make_pkt(3));
    }
}
