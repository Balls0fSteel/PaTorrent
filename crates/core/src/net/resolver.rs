// Ported from libtorrent-rasterbar 2.0.11: src/resolver.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: blocking `getaddrinfo` (std `ToSocketAddrs`) on two worker threads instead of asio's
// resolver; the cache (libtorrent `resolver_cache_timeout`, 1200 s) is shared by the workers.

//! Host name resolution off the network thread.

use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const CACHE_TIMEOUT: Duration = Duration::from_secs(1200);
const WORKERS: usize = 2;

pub(crate) struct Resolved {
    pub cookie: u64,
    pub host: String,
    pub result: Result<Vec<IpAddr>, String>,
}

type Cache = Arc<Mutex<HashMap<String, (Vec<IpAddr>, Instant)>>>;

pub(crate) struct Resolver {
    tx: Option<mpsc::Sender<(String, u64)>>,
    cache: Cache,
    handles: Vec<JoinHandle<()>>,
}

impl Resolver {
    pub fn new(done: mpsc::Sender<Resolved>, wake: Arc<dyn Fn() + Send + Sync>) -> Resolver {
        let (tx, rx) = mpsc::channel::<(String, u64)>();
        let rx = Arc::new(Mutex::new(rx));
        let cache: Cache = Arc::default();
        let handles = (0..WORKERS)
            .map(|i| {
                let (rx, cache, done, wake) = (rx.clone(), cache.clone(), done.clone(), wake.clone());
                std::thread::Builder::new()
                    .name(format!("lt-resolver-{i}"))
                    .spawn(move || {
                        loop {
                            let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                            let Ok((host, cookie)) = job else { break };
                            let result = lookup(&host);
                            if let Ok(ips) = &result {
                                cache
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .insert(host.clone(), (ips.clone(), Instant::now()));
                            }
                            if done.send(Resolved { cookie, host, result }).is_ok() {
                                wake();
                            }
                        }
                    })
                    .expect("failed to spawn the resolver thread")
            })
            .collect();
        Resolver { tx: Some(tx), cache, handles }
    }

    /// Returns the addresses right away for IP literals and cached names; otherwise queues a
    /// lookup whose answer comes back as a [`Resolved`] carrying `cookie`.
    pub fn resolve(&self, host: &str, cookie: u64) -> Option<Vec<IpAddr>> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(vec![ip]);
        }
        if let Some((ips, at)) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(host) {
            if at.elapsed() < CACHE_TIMEOUT {
                return Some(ips.clone());
            }
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send((host.to_owned(), cookie));
        }
        None
    }
}

impl Drop for Resolver {
    fn drop(&mut self) {
        self.tx = None;
        // Workers may be blocked in getaddrinfo; don't wait for them.
        self.handles.clear();
    }
}

fn lookup(host: &str) -> Result<Vec<IpAddr>, String> {
    let mut ips: Vec<IpAddr> =
        (host, 0u16).to_socket_addrs().map_err(|e| e.to_string())?.map(|a| a.ip()).collect();
    // IPv4 first: most trackers and peers are reachable over it.
    ips.sort_by_key(|ip| ip.is_ipv6());
    ips.dedup();
    if ips.is_empty() { Err("host not found".into()) } else { Ok(ips) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_cache_and_lookups() {
        let (tx, rx) = mpsc::channel();
        let r = Resolver::new(tx, Arc::new(|| {}));
        assert_eq!(r.resolve("127.0.0.1", 1), Some(vec!["127.0.0.1".parse().unwrap()]));
        assert_eq!(r.resolve("::1", 1), Some(vec!["::1".parse().unwrap()]));
        assert!(r.resolve("localhost", 5).is_none());
        let res = rx.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!((res.cookie, res.host.as_str()), (5, "localhost"));
        assert!(res.result.unwrap().iter().all(|ip| ip.is_loopback()));
        assert!(r.resolve("localhost", 6).is_some(), "cached");
    }
}
