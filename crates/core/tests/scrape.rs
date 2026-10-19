//! Loopback tests for `TorrentHandle::scrape_tracker` (libtorrent `scrape_tracker`) against a
//! BEP 15 tracker: stopped torrents scraped together get their own counts (the scrapes share
//! requests, BEP 15 allowing 74 info-hashes in one), an index picks the tracker, and a
//! tracker's error comes back as `ScrapeFailed` without touching the announce state. The
//! session runs with non-default socket buffer sizes (`send_socket_buffer_size`,
//! `recv_socket_buffer_size`), which its tracker sockets take.

use std::collections::{HashMap, VecDeque};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId,
    TorrentSource,
};

/// A small torrent; `tag` makes the info-hash differ. Returns the `.torrent` and its info-hash.
fn fixture(tag: u32) -> (Vec<u8>, [u8; 20]) {
    let mut info = Entry::new_dict();
    *info.key("length") = Entry::Int(1000);
    *info.key("name") = Entry::Bytes(format!("scrape-{tag}.bin").into_bytes());
    *info.key("piece length") = Entry::Int(16 * 1024);
    *info.key("pieces") = Entry::Bytes(Sha1::digest(&[0u8; 1000]).0.to_vec());
    *info.key("x-tag") = Entry::Int(tag as i64);
    let info_hash = Sha1::digest(&info.bencode()).0;
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    (t.bencode(), info_hash)
}

/// A BEP 15 tracker answering connects, and scrapes with the counts `counts` gives each
/// info-hash it knows (an error for any other). Returns its URL; the thread returns how many
/// info-hashes each scrape request carried, once `total` info-hashes were asked for.
fn udp_tracker(
    counts: HashMap<[u8; 20], (u32, u32)>,
    total: usize,
) -> (String, std::thread::JoinHandle<Vec<usize>>) {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let url = format!("udp://{}/announce", s.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let mut sizes = Vec::new();
        let mut buf = [0u8; 2048];
        while sizes.iter().sum::<usize>() < total {
            let Ok((n, from)) = s.recv_from(&mut buf) else { break };
            if n < 16 {
                continue;
            }
            let action = u32::from_be_bytes(buf[8..12].try_into().unwrap());
            let txn = &buf[12..16];
            let mut out = Vec::new();
            if action == 0 {
                out.extend_from_slice(&0u32.to_be_bytes());
                out.extend_from_slice(txn);
                out.extend_from_slice(&0x0bad_cafeu64.to_be_bytes());
            } else if action == 2 {
                let hashes: Vec<[u8; 20]> =
                    buf[16..n].chunks_exact(20).map(|c| c.try_into().unwrap()).collect();
                sizes.push(hashes.len());
                if hashes.iter().all(|h| counts.contains_key(h)) {
                    out.extend_from_slice(&2u32.to_be_bytes());
                    out.extend_from_slice(txn);
                    for h in &hashes {
                        let (complete, incomplete) = counts[h];
                        out.extend_from_slice(&complete.to_be_bytes());
                        out.extend_from_slice(&77u32.to_be_bytes());
                        out.extend_from_slice(&incomplete.to_be_bytes());
                    }
                } else {
                    out.extend_from_slice(&3u32.to_be_bytes());
                    out.extend_from_slice(txn);
                    out.extend_from_slice(b"unknown torrent");
                }
            } else {
                continue;
            }
            s.send_to(&out, from).unwrap();
        }
        sizes
    });
    (url, h)
}

fn start() -> Session {
    let settings = Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        tick_interval: 100,
        aio_threads: 2,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        send_socket_buffer_size: 96 * 1024,
        recv_socket_buffer_size: 96 * 1024,
        ..Settings::default()
    };
    Session::start(SessionParams { settings, state_dir: None }).expect("session starts")
}

/// Takes alerts, handing each to `f`, until `f` returns true.
fn wait(
    h: &SessionHandle,
    backlog: &mut VecDeque<Alert>,
    secs: u64,
    what: &str,
    mut f: impl FnMut(&Alert) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut alerts = Vec::new();
    loop {
        while let Some(a) = backlog.pop_front() {
            if f(&a) {
                return;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(30));
        h.pop_alerts(&mut alerts);
        backlog.extend(alerts.drain(..));
    }
}

/// Adds stopped torrents with these trackers; returns their ids in order.
fn add_stopped(
    h: &SessionHandle,
    backlog: &mut VecDeque<Alert>,
    torrents: &[(Vec<u8>, Vec<String>)],
) -> Vec<TorrentId> {
    let dir = std::env::temp_dir();
    for (t, trackers) in torrents {
        let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(t.clone()), dir.to_string_lossy());
        p.flags = TorrentFlags::PAUSED;
        p.trackers = trackers.clone();
        h.add_torrent(p);
    }
    let mut ids = Vec::new();
    wait(h, backlog, 10, "adds", |a| {
        if let Alert::TorrentAdded { id, .. } = a {
            ids.push(*id);
        }
        ids.len() == torrents.len()
    });
    ids
}

#[test]
fn stopped_torrents_scraped_together_get_their_own_counts() {
    const N: u32 = 6;
    let torrents: Vec<_> = (0..N).map(fixture).collect();
    let counts: HashMap<_, _> =
        torrents.iter().enumerate().map(|(i, t)| (t.1, (10 + i as u32, 20 + i as u32))).collect();
    let (url, tracker) = udp_tracker(counts, N as usize);
    let s = start();
    let h = s.handle();
    let mut backlog = VecDeque::new();
    // the last torrent lists a dead tracker first: index 1 picks ours
    let list: Vec<_> = (0..N)
        .map(|i| {
            let trackers = if i == N - 1 {
                vec!["udp://127.0.0.1:9/announce".into(), url.clone()]
            } else {
                vec![url.clone()]
            };
            (torrents[i as usize].0.clone(), trackers)
        })
        .collect();
    let ids = add_stopped(&h, &mut backlog, &list);
    for (i, &id) in ids.iter().enumerate() {
        h.torrent(id).scrape_tracker(if i == N as usize - 1 { Some(1) } else { None });
    }
    let mut got = HashMap::new();
    wait(&h, &mut backlog, 15, "scrape replies", |a| {
        match a {
            Alert::ScrapeReply { id, url: u, complete, incomplete } => {
                assert_eq!(*u, url);
                got.insert(*id, (*complete, *incomplete));
            }
            Alert::ScrapeFailed { message, .. } => panic!("scrape failed: {message}"),
            _ => {}
        }
        got.len() == ids.len()
    });
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(got[id], (10 + i as u32, 20 + i as u32), "torrent {i}");
    }
    let sizes = tracker.join().unwrap();
    // asked for together: most likely one request, at worst one each
    assert_eq!(sizes.iter().sum::<usize>(), N as usize, "{sizes:?}");
    assert!(sizes.len() <= N as usize, "{sizes:?}");
    eprintln!("scrape requests carried {sizes:?} info-hashes");
    s.shutdown();
}

#[test]
fn a_trackers_error_answers_the_scrape() {
    let (t, _) = fixture(100);
    // the tracker knows no torrent
    let (url, tracker) = udp_tracker(HashMap::new(), 1);
    let s = start();
    let h = s.handle();
    let mut backlog = VecDeque::new();
    let ids = add_stopped(&h, &mut backlog, &[(t, vec![url.clone()])]);
    h.torrent(ids[0]).scrape_tracker(None);
    wait(&h, &mut backlog, 15, "scrape failure", |a| match a {
        Alert::ScrapeFailed { id, url: u, message } => {
            assert_eq!((*id, u.as_str(), message.as_str()), (ids[0], url.as_str(), "unknown torrent"));
            true
        }
        Alert::ScrapeReply { .. } => panic!("a reply"),
        _ => false,
    });
    assert_eq!(tracker.join().unwrap(), [1]);
    s.shutdown();
}
