// Ported from libtorrent-rasterbar 2.0.11: src/kademlia/dos_blocker.cpp,
// include/libtorrent/kademlia/dos_blocker.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: "no limit yet" is `None` instead of `min_time()`; banning is logged by the caller.

//! Ignores IPs that flood us with DHT messages.

use std::net::IpAddr;
use std::time::{Duration, Instant};

const NUM_BAN_NODES: usize = 20;

#[derive(Clone, Copy, Debug)]
struct NodeBanEntry {
    src: Option<IpAddr>,
    limit: Option<Instant>,
    count: i32,
}

/// libtorrent `dos_blocker`: tracks the 20 most active IPs.
#[derive(Clone, Debug)]
pub struct DosBlocker {
    ban_nodes: [NodeBanEntry; NUM_BAN_NODES],
    /// Messages per second allowed from one IP.
    message_rate_limit: i32,
    /// Seconds a flooding IP stays blocked.
    block_timeout: i32,
}

impl Default for DosBlocker {
    fn default() -> DosBlocker {
        DosBlocker {
            ban_nodes: [NodeBanEntry { src: None, limit: None, count: 0 }; NUM_BAN_NODES],
            message_rate_limit: 5,
            block_timeout: 5 * 60,
        }
    }
}

impl DosBlocker {
    pub fn set_rate_limit(&mut self, l: i32) {
        self.message_rate_limit = l;
    }

    pub fn set_block_timer(&mut self, t: i32) {
        self.block_timeout = t;
    }

    /// Returns false if the message should be dropped.
    pub fn incoming(&mut self, addr: IpAddr, now: Instant) -> bool {
        let mut min = 0;
        let mut found = None;
        for (i, e) in self.ban_nodes.iter().enumerate() {
            if e.src == Some(addr) {
                found = Some(i);
                break;
            }
            let m = &self.ban_nodes[min];
            if e.count < m.count || (e.count == m.count && e.limit < m.limit) {
                min = i;
            }
        }
        let limit = self.message_rate_limit * 10;
        match found {
            Some(i) => {
                let e = &mut self.ban_nodes[i];
                e.count += 1;
                if e.count >= limit {
                    if e.limit.is_some_and(|l| now < l) {
                        if e.count == limit {
                            // too many messages within 10 seconds: ignore it until it has been
                            // quiet for the block timeout
                            e.limit = Some(now + Duration::from_secs(self.block_timeout.max(0) as u64));
                        }
                        return false;
                    }
                    // the messages took more than 10 seconds: start over
                    e.count = 0;
                    e.limit = Some(now + Duration::from_secs(10));
                }
            }
            None => {
                self.ban_nodes[min] =
                    NodeBanEntry { src: Some(addr), limit: Some(now + Duration::from_secs(10)), count: 1 };
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::ip_voter::tests::rand_v4;

    // libtorrent test_dos_blocker.cpp
    #[test]
    fn dos_blocker() {
        let mut b = DosBlocker::default();
        let spammer: IpAddr = "10.10.10.10".parse().unwrap();
        let mut now = Instant::now();
        for _ in 0..1000 {
            b.incoming(spammer, now);
            now += Duration::from_millis(1);
            assert!(b.incoming(rand_v4(), now));
            now += Duration::from_millis(1);
        }
        now += Duration::from_millis(1);
        assert!(!b.incoming(spammer, now));
    }
}
