// Ported from libtorrent-rasterbar 2.0.11: include/libtorrent/sliding_average.hpp, src/stat.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: `stat_channel` keeps a running byte counter plus an exponential average updated per
// tick with the real elapsed time, instead of a fixed one-second second_tick.

//! Transfer counters and rate averages.

use std::time::{SystemTime, UNIX_EPOCH};

/// libtorrent `sliding_average<int, N>`: an exponential moving average that warms up over its
/// first `inverted_gain` samples, so early values are not dragged toward zero.
#[derive(Clone, Copy, Debug, Default)]
pub struct SlidingAverage<const N: i64> {
    /// Fixed point, 6 fractional bits (as libtorrent).
    mean: i64,
    average_deviation: i64,
    num_samples: i64,
}

impl<const N: i64> SlidingAverage<N> {
    pub fn add_sample(&mut self, s: i64) {
        let s = s * 64;
        let deviation = if self.num_samples > 0 { (self.mean - s).abs() } else { 0 };
        if self.num_samples < N {
            self.num_samples += 1;
        }
        self.mean += (s - self.mean) / self.num_samples;
        if self.num_samples > 1 {
            self.average_deviation += (deviation - self.average_deviation) / (self.num_samples - 1);
        }
    }

    pub fn mean(&self) -> i64 {
        if self.num_samples > 0 { (self.mean + 32) / 64 } else { 0 }
    }

    pub fn avg_deviation(&self) -> i64 {
        if self.num_samples > 1 { (self.average_deviation + 32) / 64 } else { 0 }
    }
}

/// One direction of traffic: a lifetime counter plus a rate in bytes per second, averaged
/// over roughly five seconds (libtorrent `stat_channel`).
#[derive(Clone, Copy, Debug, Default)]
pub struct StatChannel {
    pub total: u64,
    counter: u64,
    rate: f64,
    warm: bool,
}

impl StatChannel {
    pub fn add(&mut self, bytes: u64) {
        self.total += bytes;
        self.counter += bytes;
    }

    /// Folds the bytes counted since the last call into the rate. `dt` is in seconds.
    pub fn tick(&mut self, dt: f64) {
        if dt <= 0.0 {
            return;
        }
        let sample = self.counter as f64 / dt;
        self.counter = 0;
        // Five-second time constant, independent of the tick length.
        let alpha = (dt / 5.0).min(1.0);
        if self.warm {
            self.rate += (sample - self.rate) * alpha;
        } else {
            self.rate = sample;
            self.warm = true;
        }
        if self.rate < 1.0 {
            self.rate = 0.0;
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate.min(u32::MAX as f64) as u32
    }

    /// Forgets the rate (the lifetime total stays): a channel that will not tick for a while,
    /// such as a stopped torrent's, would otherwise report its last rate for ever.
    pub fn reset_rate(&mut self) {
        self.counter = 0;
        self.rate = 0.0;
        self.warm = false;
    }
}

/// Payload and protocol traffic in both directions (libtorrent `stat`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Stat {
    pub download_payload: StatChannel,
    pub upload_payload: StatChannel,
    pub download_protocol: StatChannel,
    pub upload_protocol: StatChannel,
}

impl Stat {
    pub fn tick(&mut self, dt: f64) {
        self.download_payload.tick(dt);
        self.upload_payload.tick(dt);
        self.download_protocol.tick(dt);
        self.upload_protocol.tick(dt);
    }

    pub fn received(&mut self, payload: u64, protocol: u64) {
        self.download_payload.add(payload);
        self.download_protocol.add(protocol);
    }

    pub fn sent(&mut self, payload: u64, protocol: u64) {
        self.upload_payload.add(payload);
        self.upload_protocol.add(protocol);
    }

    /// Every channel's rate back to zero; the totals stay. For a torrent that stops: its stat
    /// ticks only while it runs, so the rates would stay at their last values.
    pub fn reset_rates(&mut self) {
        self.download_payload.reset_rate();
        self.upload_payload.reset_rate();
        self.download_protocol.reset_rate();
        self.upload_protocol.reset_rate();
    }

    pub fn download_rate(&self) -> u32 {
        self.download_payload.rate().saturating_add(self.download_protocol.rate())
    }

    pub fn upload_rate(&self) -> u32 {
        self.upload_payload.rate().saturating_add(self.upload_protocol.rate())
    }

    pub fn total_download(&self) -> u64 {
        self.download_payload.total + self.download_protocol.total
    }

    pub fn total_upload(&self) -> u64 {
        self.upload_payload.total + self.upload_protocol.total
    }
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // libtorrent test_sliding_average.cpp
    const SAMPLES: [i64; 128] = [
        49, 51, 60, 46, 65, 53, 76, 59, 57, 54, 56, 51, 45, 80, 53, 62, 69, 67, 66, 56, 56, 61, 52, 61, 61,
        62, 59, 53, 48, 68, 47, 47, 63, 51, 53, 54, 46, 65, 64, 64, 45, 68, 64, 66, 53, 42, 57, 58, 57, 47,
        55, 59, 64, 61, 37, 67, 55, 52, 60, 60, 44, 57, 50, 77, 56, 54, 49, 68, 66, 64, 47, 60, 46, 47, 81,
        74, 65, 62, 44, 75, 65, 43, 58, 59, 53, 67, 49, 51, 33, 47, 49, 50, 54, 48, 55, 80, 67, 51, 66, 52,
        48, 57, 30, 51, 72, 65, 78, 56, 74, 68, 49, 66, 63, 57, 61, 62, 64, 62, 61, 52, 67, 64, 59, 61, 69,
        60, 54, 69,
    ];

    #[test]
    fn sliding_average_reaction_and_convergence() {
        let mut avg = SlidingAverage::<10>::default();
        avg.add_sample(-10);
        avg.add_sample(10);
        assert_eq!(avg.mean(), 0);

        let mut avg = SlidingAverage::<10>::default();
        avg.add_sample(10);
        avg.add_sample(20);
        assert_eq!(avg.mean(), 15);

        let mut avg = SlidingAverage::<10>::default();
        avg.add_sample(100);
        (0..20).for_each(|_| avg.add_sample(10));
        assert!((avg.mean() - 10).abs() <= 3);

        let mut avg = SlidingAverage::<10>::default();
        avg.add_sample(-100);
        (0..20).for_each(|_| avg.add_sample(-10));
        assert!((avg.mean() + 10).abs() <= 3);

        let mut avg = SlidingAverage::<10>::default();
        SAMPLES.iter().for_each(|&s| avg.add_sample(s));
        assert!((avg.mean() - 60).abs() <= 3);

        let mut avg = SlidingAverage::<4>::default();
        assert_eq!((avg.mean(), avg.avg_deviation()), (0, 0));
        avg.add_sample(500);
        assert_eq!((avg.mean(), avg.avg_deviation()), (500, 0));
        avg.add_sample(501);
        assert_eq!(avg.avg_deviation(), 1);
        avg.add_sample(0);
        avg.add_sample(0);
        assert!((avg.mean() - 250).abs() < 50);
        assert!((avg.avg_deviation() - 250).abs() < 80);
    }

    #[test]
    fn stat_channel_rate() {
        let mut c = StatChannel::default();
        c.add(1000);
        c.tick(1.0);
        assert_eq!(c.rate(), 1000, "first sample sets the rate");
        for _ in 0..50 {
            c.add(500);
            c.tick(0.5);
        }
        assert!((990..=1010).contains(&c.rate()), "{}", c.rate());
        for _ in 0..100 {
            c.tick(0.5);
        }
        assert_eq!(c.rate(), 0);
        assert_eq!(c.total, 1000 + 50 * 500);
    }

    #[test]
    fn reset_rates_keeps_the_totals() {
        let mut s = Stat::default();
        s.received(1_009_000, 100);
        s.sent(2_000, 50);
        s.tick(1.0);
        assert_eq!(s.download_payload.rate(), 1_009_000);
        s.reset_rates();
        assert_eq!((s.download_rate(), s.upload_rate()), (0, 0));
        assert_eq!((s.total_download(), s.total_upload()), (1_009_100, 2_050));
        // the next tick starts afresh rather than averaging with the old rate
        s.received(400, 0);
        s.tick(1.0);
        assert_eq!(s.download_payload.rate(), 400);
    }
}
