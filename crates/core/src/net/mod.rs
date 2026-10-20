//! Platform networking helpers: resolver, OS TLS, interface enumeration and raw socket options,
//! listen-interface parsing and expansion, network-change notifications, bandwidth buckets, and
//! the blocking HTTP(S) fetch helper frontends use ([`fetch`]).

pub(crate) mod enum_net;
pub mod fetch;
pub(crate) mod ip_filter;
pub(crate) mod ip_notifier;
pub(crate) mod ip_voter;
pub(crate) mod listen;
pub(crate) mod resolver;
pub(crate) mod sys;
pub mod tls;

/// A token bucket (libtorrent `bandwidth_channel`): `limit` bytes per second, refilled in small
/// steps; 0 means unlimited.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Bucket {
    pub limit: u32,
    quota: i64,
}

impl Bucket {
    pub fn new(limit: u32) -> Bucket {
        Bucket { limit, quota: limit as i64 / 10 }
    }

    pub fn set_limit(&mut self, limit: u32) {
        self.limit = limit;
        self.quota = self.quota.min(limit as i64);
    }

    /// Adds `dt` seconds worth of quota, keeping at most a quarter second of burst.
    pub fn refill(&mut self, dt: f64) {
        if self.limit > 0 {
            let cap = (self.limit as i64 / 4).max(16 * 1024);
            self.quota = (self.quota + (self.limit as f64 * dt) as i64).min(cap);
        }
    }

    /// How many bytes may move now (`usize::MAX` when unlimited).
    pub fn available(&self) -> usize {
        if self.limit == 0 { usize::MAX } else { self.quota.max(0) as usize }
    }

    pub fn consume(&mut self, n: usize) {
        if self.limit > 0 {
            self.quota -= n as i64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket() {
        let mut b = Bucket::new(0);
        assert_eq!(b.available(), usize::MAX);
        b = Bucket::new(100_000);
        assert_eq!(b.available(), 10_000);
        b.consume(15_000);
        assert_eq!(b.available(), 0);
        b.refill(0.1);
        assert_eq!(b.available(), 5_000);
        b.refill(10.0);
        assert_eq!(b.available(), 25_000, "burst is capped");
    }
}
