// Ported from libtorrent-rasterbar 2.0.11: src/timestamp_history.cpp, include/libtorrent/aux_/timestamp_history.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: none beyond the language.

//! The lowest one-way delay samples seen in the last 20 minutes: the base the congestion
//! controller measures queueing delay against.

use super::compare_less_wrap;

const HISTORY_SIZE: usize = 20;
const NOT_INITIALIZED: u16 = 0xffff;
const TIME_MASK: u32 = 0xffff_ffff;

#[derive(Clone, Debug)]
pub(crate) struct TimestampHistory {
    /// A circular buffer: the lowest sample of each of the last `HISTORY_SIZE` minutes.
    history: [u32; HISTORY_SIZE],
    /// The lowest sample in the history.
    base: u32,
    index: u16,
    /// Samples since the last minute step; `NOT_INITIALIZED` before the first sample.
    num_samples: u16,
}

impl Default for TimestampHistory {
    fn default() -> TimestampHistory {
        TimestampHistory { history: [0; HISTORY_SIZE], base: 0, index: 0, num_samples: NOT_INITIALIZED }
    }
}

impl TimestampHistory {
    pub fn initialized(&self) -> bool {
        self.num_samples != NOT_INITIALIZED
    }

    pub fn base(&self) -> u32 {
        debug_assert!(self.initialized());
        self.base
    }

    /// Adds a sample and returns its delay over the base. `step` is true once a minute.
    pub fn add_sample(&mut self, sample: u32, step: bool) -> u32 {
        if !self.initialized() {
            self.history = [sample; HISTORY_SIZE];
            self.base = sample;
            self.num_samples = 0;
        }
        // don't let the counter wrap
        if self.num_samples < 0xfffe {
            self.num_samples += 1;
        }
        let i = self.index as usize;
        if compare_less_wrap(sample, self.base, TIME_MASK) {
            // a new low: the base and this minute's entry follow it
            self.base = sample;
            self.history[i] = sample;
        } else if compare_less_wrap(sample, self.history[i], TIME_MASK) {
            self.history[i] = sample;
        }
        let ret = sample.wrapping_sub(self.base);
        // Don't step the base delay history unless we have at least 120 samples. Anything less
        // suggests the connection is essentially idle and the samples not very reliable.
        if step && self.num_samples > 120 {
            self.num_samples = 0;
            self.index = ((self.index as usize + 1) % HISTORY_SIZE) as u16;
            self.history[self.index as usize] = sample;
            self.base = sample;
            for &h in &self.history {
                if compare_less_wrap(h, self.base, TIME_MASK) {
                    self.base = h;
                }
            }
        }
        ret
    }

    /// Shifts the base (clock-drift compensation) and makes the history agree with it.
    pub fn adjust_base(&mut self, change: i32) {
        debug_assert!(self.initialized());
        self.base = self.base.wrapping_add(change as u32);
        for h in &mut self.history {
            if compare_less_wrap(*h, self.base, TIME_MASK) {
                *h = self.base;
            }
        }
    }
}

// libtorrent test/test_timestamp_history.cpp
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_history() {
        let mut h = TimestampHistory::default();
        assert_eq!(h.add_sample(0x32, false), 0);
        assert_eq!(h.base(), 0x32);
        assert_eq!(h.add_sample(0x33, false), 0x1);
        assert_eq!(h.base(), 0x32);
        assert_eq!(h.add_sample(0x3433, false), 0x3401);
        assert_eq!(h.base(), 0x32);
        assert_eq!(h.add_sample(0x30, false), 0);
        assert_eq!(h.base(), 0x30);
        // wrapping of the timestamp is handled
        h.add_sample(0xfffffff3, false);
        assert_eq!(h.base(), 0xfffffff3);
    }

    /// The two cases libtorrent's test leaves as TODOs: a minute step with enough samples moves
    /// the base to the lowest of the last 20 minutes, and a sample between the base and this
    /// minute's entry lowers only the entry.
    #[test]
    fn stepping_and_minute_entries() {
        let mut h = TimestampHistory::default();
        h.add_sample(1000, false);
        for _ in 0..130 {
            h.add_sample(1500, false);
        }
        // a sample below the minute entry but above the base changes the entry only
        assert_eq!(h.add_sample(1200, false), 200);
        assert_eq!(h.base(), 1000);
        // the step closes the minute: the new minute starts at this sample, and the base is the
        // lowest entry of all minutes, which is still the old 1000
        assert_eq!(h.add_sample(1300, true), 300);
        assert_eq!(h.base(), 1000);
        // 20 more steps and the 1000 falls off the history
        for _ in 0..HISTORY_SIZE {
            for _ in 0..125 {
                h.add_sample(2000, false);
            }
            h.add_sample(2000, true);
        }
        assert_eq!(h.base(), 2000);
        h.adjust_base(50);
        assert_eq!(h.base(), 2050);
        assert_eq!(h.add_sample(2060, false), 10);
    }
}
