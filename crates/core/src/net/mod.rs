//! Platform networking helpers: resolver, listen-interface parsing, bandwidth buckets.

pub(crate) mod resolver;

use std::net::SocketAddr;

/// Parses libtorrent's `listen_interfaces` (`"0.0.0.0:6881,[::]:6881"`). Entries that are not
/// `ip:port` (interface names, which libtorrent also accepts) are skipped for now.
pub(crate) fn parse_listen_interfaces(s: &str) -> Vec<SocketAddr> {
    s.split(',')
        .map(str::trim)
        .filter_map(|e| {
            let e = e.trim_end_matches('s'); // "s" suffix marks SSL listen sockets
            e.parse::<SocketAddr>().ok()
        })
        .collect()
}

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
    fn listen_interfaces() {
        assert_eq!(
            parse_listen_interfaces("0.0.0.0:6881, [::]:6881,eth0:5,127.0.0.1:0s"),
            [
                "0.0.0.0:6881".parse::<SocketAddr>().unwrap(),
                "[::]:6881".parse().unwrap(),
                "127.0.0.1:0".parse().unwrap()
            ]
        );
    }

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
