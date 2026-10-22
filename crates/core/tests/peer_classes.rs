// Ported from libtorrent-rasterbar 2.0.11: test/test_peer_classes.cpp (session_peer_class_filter,
// session_peer_class_type_filter; its `peer_class` pool test is a unit test in
// engine/peer_class.rs, the pool being internal)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the loopback swarms below are new: the global, per-torrent and class limits hold
// with peer classes underneath, uTP peers taken out of the global class by the type filter and
// local-class peers escape the global limit, a mixed TCP and uTP swarm completes under
// `peer_proportional`.

//! Peer classes through the session API (libtorrent `create_peer_class`,
//! `set_peer_class_filter`, `set_peer_class_type_filter` and the rest) and in loopback swarms.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, IpFilter, MixedModeAlgorithm, PeerClassId, PeerClassInfo, PeerClassTypeFilter,
    PeerFlags, Session, SessionHandle, SessionParams, Settings, SocketType, TorrentFlags, TorrentId,
    TorrentSource, TorrentState, TorrentStatus,
};

const PIECE: usize = 32 * 1024;
/// Bytes in the fixture's files.
const TOTAL: usize = 300_000 + 1_000_000 + 12_345;

struct Fixture {
    torrent: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
}

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("lt-classes-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A three-file torrent of about 1.3 MB; `tag` makes the info-hash differ between tests.
fn fixture(tag: &str) -> Fixture {
    let sizes = [("alpha.bin", 300_000usize), ("sub/beta.bin", 1_000_000), ("sub/gamma.txt", 12_345)];
    let mut all = Vec::new();
    let mut files = Vec::new();
    for (i, (name, size)) in sizes.iter().enumerate() {
        let mut x = 0x9e3779b9u32.wrapping_mul(i as u32 + 1);
        let data: Vec<u8> = (0..*size)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        all.extend_from_slice(&data);
        files.push((name.to_string(), data));
    }
    let pieces: Vec<u8> = all.chunks(PIECE).flat_map(|c| Sha1::digest(c).0).collect();
    let mut info = Entry::new_dict();
    let mut list = Entry::new_list();
    for (name, data) in &files {
        let mut f = Entry::new_dict();
        *f.key("length") = Entry::Int(data.len() as i64);
        *f.key("path") = Entry::List(name.split('/').map(|p| Entry::Bytes(p.as_bytes().to_vec())).collect());
        list.list_mut().push(f);
    }
    *info.key("files") = list;
    *info.key("name") = Entry::Bytes(b"classes-test".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    *info.key("x-tag") = Entry::Bytes(tag.as_bytes().to_vec());
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    Fixture { torrent: t.bencode(), files }
}

fn write_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let p = dir.join("classes-test").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

fn assert_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let got =
            std::fs::read(dir.join("classes-test").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(got == *data, "{name} differs");
    }
}

fn settings() -> Settings {
    Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        tick_interval: 100,
        aio_threads: 2,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    }
}

fn start_with(f: impl FnOnce(&mut Settings)) -> Session {
    let mut s = settings();
    f(&mut s);
    Session::start(SessionParams { settings: s, state_dir: None }).expect("session starts")
}

fn tcp_only(s: &mut Settings) {
    s.enable_incoming_utp = false;
    s.enable_outgoing_utp = false;
}

fn utp_only(s: &mut Settings) {
    s.enable_incoming_tcp = false;
    s.enable_outgoing_tcp = false;
}

/// A session handle plus the alerts popped but not looked at yet.
struct Watch {
    h: SessionHandle,
    backlog: VecDeque<Alert>,
}

impl Watch {
    fn new(h: SessionHandle) -> Watch {
        Watch { h, backlog: VecDeque::new() }
    }
}

fn wait<T>(w: &mut Watch, secs: u64, what: &str, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut alerts = Vec::new();
    let mut seen = Vec::new();
    loop {
        while let Some(a) = w.backlog.pop_front() {
            if !matches!(a, Alert::StateUpdate(_)) {
                seen.push(a.message());
            }
            if let Some(v) = f(&a) {
                return v;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        w.h.post_torrent_updates();
        std::thread::sleep(Duration::from_millis(30));
        w.h.pop_alerts(&mut alerts);
        w.backlog.extend(alerts.drain(..));
    }
    panic!("timed out waiting for {what}; alerts: {seen:#?}");
}

fn listen_port(w: &mut Watch) -> u16 {
    wait(w, 5, "listen", |a| match a {
        Alert::ListenSucceeded { address, .. } => Some(address.port()),
        _ => None,
    })
}

fn add(w: &mut Watch, f: &Fixture, dir: &Path) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    w.h.add_torrent(p);
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

fn status_of(a: &Alert, id: TorrentId) -> Option<TorrentStatus> {
    match a {
        Alert::StateUpdate(v) => v.iter().find(|s| s.id == id).cloned(),
        _ => None,
    }
}

fn wait_state(w: &mut Watch, id: TorrentId, state: TorrentState, secs: u64) -> TorrentStatus {
    wait(w, secs, &format!("{state:?}"), |a| {
        status_of(a, id).filter(|s| s.state == state && s.error.is_empty())
    })
}

/// A seeding session of `f`, with `cfg` applied to its settings. Returns it, its directory and
/// its port.
fn seeder(f: &Fixture, cfg: impl FnOnce(&mut Settings)) -> (Session, PathBuf, u16) {
    let dir = temp_dir("seed");
    write_files(f, &dir);
    let s = start_with(cfg);
    let mut w = Watch::new(s.handle());
    let port = listen_port(&mut w);
    let id = add(&mut w, f, &dir);
    wait_state(&mut w, id, TorrentState::Seeding, 20);
    (s, dir, port)
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// Downloads `f` from seeds at `ports` into a new leech session (`cfg` applies to its
/// settings, `prepare` runs once the torrent is added), and returns how long it took from the
/// first connect to the finished download, failing after `secs`.
fn timed_download(
    f: &Fixture,
    ports: &[u16],
    cfg: impl FnOnce(&mut Settings),
    prepare: impl FnOnce(&SessionHandle, TorrentId),
    secs: u64,
) -> Duration {
    let dir = temp_dir("leech");
    let leech = start_with(cfg);
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, f, &dir);
    prepare(&leech.handle(), id);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    let started = Instant::now();
    for &port in ports {
        leech.handle().torrent(id).connect_peer(loopback(port));
    }
    wait(&mut w, secs, "finished", |a| {
        matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(())
    });
    let took = started.elapsed();
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert_eq!(st.total_wanted_done, st.total_wanted);
    leech.shutdown();
    assert_files(f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    took
}

/// The least time a download of the fixture at `limit` bytes per second can take: the bucket
/// starts with a tenth of a second of quota and bursts at most a quarter of a second, so say a
/// second less than the plain division, and another second for loaded test runners.
fn at_least(limit: usize) -> Duration {
    Duration::from_secs_f64(TOTAL as f64 / limit as f64 - 2.0)
}

// --- The session API (libtorrent test_peer_classes.cpp) ---------------------------------------

#[test]
fn session_peer_class_filter() {
    let ses = start_with(|_| {});
    let h = ses.handle();
    let my_class = h.create_peer_class("200.1.x.x IP range").unwrap();

    let mut f = IpFilter::new();
    f.add_rule(ip("200.1.1.0"), ip("200.1.255.255"), my_class.mask());
    h.set_peer_class_filter(f.clone());

    assert_eq!(h.get_peer_class_filter().unwrap().export_filter().0, f.export_filter().0);
}

#[test]
fn session_peer_class_type_filter() {
    let ses = start_with(|_| {});
    let h = ses.handle();
    let my_class = h.create_peer_class("all utp sockets").unwrap();

    let mut f = PeerClassTypeFilter::new();
    f.add(SocketType::Utp, my_class);
    f.disallow(SocketType::Utp, PeerClassId::GLOBAL);
    h.set_peer_class_type_filter(f.clone());

    assert_eq!(h.get_peer_class_type_filter().unwrap(), f);
}

#[test]
fn the_default_classes_and_filters() {
    let ses = start_with(|s| {
        s.download_rate_limit = 123_456;
        s.upload_rate_limit = 65_432;
    });
    let h = ses.handle();
    let global = h.get_peer_class(PeerClassId::GLOBAL).unwrap();
    assert_eq!(global.label, "global");
    assert_eq!((global.download_limit, global.upload_limit), (123_456, 65_432));
    assert_eq!((global.upload_priority, global.download_priority), (1, 1));
    assert!(!global.ignore_unchoke_slots);
    assert_eq!(global.connection_limit_factor, 100);
    assert_eq!(h.get_peer_class(PeerClassId::TCP).unwrap().label, "tcp");
    let local = h.get_peer_class(PeerClassId::LOCAL).unwrap();
    assert_eq!(local.label, "local");
    assert!(local.ignore_unchoke_slots);
    assert_eq!(local.connection_limit_factor, 150);
    assert_eq!(h.get_peer_class(PeerClassId(3)), None);

    // every address in the global class (libtorrent `init_peer_class_filter(false)`)
    let f = h.get_peer_class_filter().unwrap();
    for a in ["127.0.0.1", "10.1.2.3", "192.168.0.1", "8.8.8.8", "::1", "fe80::1", "2001:db8::1"] {
        assert_eq!(f.access(ip(a)), PeerClassId::GLOBAL.mask(), "{a}");
    }
    // TCP sockets join the tcp class
    let mut t = PeerClassTypeFilter::new();
    t.add(SocketType::Tcp, PeerClassId::TCP);
    t.add(SocketType::SslTcp, PeerClassId::TCP);
    t.add(SocketType::I2p, PeerClassId::TCP);
    assert_eq!(h.get_peer_class_type_filter().unwrap(), t);
    assert_eq!(Settings::default().mixed_mode_algorithm, MixedModeAlgorithm::PeerProportional);
}

#[test]
fn classes_are_created_changed_and_deleted() {
    let ses = start_with(|_| {});
    let h = ses.handle();
    let c = h.create_peer_class("mine").unwrap();
    assert_eq!(c, PeerClassId(3));
    let info = PeerClassInfo {
        ignore_unchoke_slots: true,
        connection_limit_factor: 200,
        label: "renamed".into(),
        upload_limit: 5,
        download_limit: 40_000,
        upload_priority: 0,
        download_priority: 300,
    };
    h.set_peer_class(c, info.clone());
    let got = h.get_peer_class(c).unwrap();
    // limits of 1 to 9 become 10, priorities are clamped to 1..=255
    assert_eq!(got, PeerClassInfo { upload_limit: 10, upload_priority: 1, download_priority: 255, ..info });

    let d = h.create_peer_class("other").unwrap();
    assert_eq!(d, PeerClassId(4));
    h.delete_peer_class(c);
    assert_eq!(h.get_peer_class(c), None);
    // its id is reused
    assert_eq!(h.create_peer_class("again").unwrap(), c);
    assert_eq!(h.get_peer_class(c).unwrap().label, "again");
    assert_eq!(h.get_peer_class(c).unwrap().download_limit, 0);

    // the default classes stay
    h.delete_peer_class(PeerClassId::GLOBAL);
    h.delete_peer_class(PeerClassId::LOCAL);
    assert!(h.get_peer_class(PeerClassId::GLOBAL).is_some());
    assert!(h.get_peer_class(PeerClassId::LOCAL).is_some());
}

#[test]
fn the_global_class_is_the_session_rate_limit() {
    let ses = start_with(|_| {});
    let h = ses.handle();
    let mut info = h.get_peer_class(PeerClassId::GLOBAL).unwrap();
    info.download_limit = 77_000;
    h.set_peer_class(PeerClassId::GLOBAL, info);
    assert_eq!(h.get_peer_class(PeerClassId::GLOBAL).unwrap().download_limit, 77_000);
    // settings that leave the rate limits alone leave the class alone (libtorrent applies
    // `download_rate_limit` only when it changes)
    h.apply_settings(Settings { connections_limit: 150, ..settings() });
    assert_eq!(h.get_peer_class(PeerClassId::GLOBAL).unwrap().download_limit, 77_000);
    h.apply_settings(Settings { download_rate_limit: 55_000, ..settings() });
    assert_eq!(h.get_peer_class(PeerClassId::GLOBAL).unwrap().download_limit, 55_000);
}

#[test]
fn calls_after_shutdown_answer_none() {
    let ses = start_with(|_| {});
    let h = ses.handle();
    ses.shutdown();
    assert_eq!(h.create_peer_class("late"), None);
    assert_eq!(h.get_peer_class(PeerClassId::GLOBAL), None);
    assert_eq!(h.get_peer_class_filter(), None);
    assert_eq!(h.get_peer_class_type_filter(), None);
}

// --- Rate limits through classes in loopback swarms -------------------------------------------

#[test]
fn the_global_limit_holds_with_classes_underneath() {
    let f = fixture("global");
    let (seed, seed_dir, port) = seeder(&f, |_| {});
    let limit = 300_000;
    let took = timed_download(&f, &[port], |s| s.download_rate_limit = limit as u32, |_, _| {}, 60);
    assert!(took >= at_least(limit), "{took:?} at {limit} B/s");
    seed.shutdown();
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn the_seed_upload_limit_holds_with_classes_underneath() {
    let f = fixture("upload");
    let limit = 300_000;
    let (seed, seed_dir, port) = seeder(&f, |s| s.upload_rate_limit = limit as u32);
    let took = timed_download(&f, &[port], |_| {}, |_, _| {}, 60);
    assert!(took >= at_least(limit), "{took:?} at {limit} B/s");
    seed.shutdown();
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_torrent_limit_holds_with_classes_underneath() {
    let f = fixture("torrent");
    let (seed, seed_dir, port) = seeder(&f, |_| {});
    let limit = 300_000;
    let took =
        timed_download(&f, &[port], |_| {}, |h, id| h.torrent(id).set_download_limit(limit as u32), 60);
    assert!(took >= at_least(limit), "{took:?} at {limit} B/s");
    seed.shutdown();
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_class_from_the_peer_class_filter_limits_its_peers() {
    let f = fixture("filter");
    let (seed, seed_dir, port) = seeder(&f, |_| {});
    let limit = 300_000;
    let took = timed_download(
        &f,
        &[port],
        |_| {},
        |h, _| {
            let c = h.create_peer_class("loopback").unwrap();
            let mut info = h.get_peer_class(c).unwrap();
            info.download_limit = limit as u32;
            h.set_peer_class(c, info);
            let mut filter = IpFilter::new();
            filter.add_rule(ip("127.0.0.0"), ip("127.255.255.255"), c.mask());
            h.set_peer_class_filter(filter);
        },
        60,
    );
    assert!(took >= at_least(limit), "{took:?} at {limit} B/s");
    seed.shutdown();
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn utp_peers_taken_out_of_the_global_class_skip_its_limit() {
    // libtorrent's deprecated `rate_limit_utp = false`: the global class disallowed for uTP
    // sockets. At 40 kB/s the download would take half a minute.
    let f = fixture("utp");
    let (seed, seed_dir, port) = seeder(&f, utp_only);
    timed_download(
        &f,
        &[port],
        |s| {
            utp_only(s);
            s.download_rate_limit = 40_000;
        },
        |h, _| {
            let mut t = h.get_peer_class_type_filter().unwrap();
            t.disallow(SocketType::Utp, PeerClassId::GLOBAL);
            h.set_peer_class_type_filter(t);
        },
        15,
    );
    seed.shutdown();
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn local_class_peers_skip_the_global_limit() {
    // libtorrent's default filter: loopback in the local class only, outside the global limit
    let f = fixture("local");
    let (seed, seed_dir, port) = seeder(&f, |_| {});
    timed_download(
        &f,
        &[port],
        |s| s.download_rate_limit = 40_000,
        |h, _| {
            let mut filter = IpFilter::new();
            filter.add_rule(ip("0.0.0.0"), ip("255.255.255.255"), PeerClassId::GLOBAL.mask());
            filter.add_rule(ip("127.0.0.0"), ip("127.255.255.255"), PeerClassId::LOCAL.mask());
            h.set_peer_class_filter(filter);
        },
        15,
    );
    seed.shutdown();
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_mixed_tcp_and_utp_swarm_completes_under_peer_proportional() {
    // Five uTP-only seeds uploading at 100 kB/s each and one TCP-only seed: while five or more
    // of the leech's peers are busy and some are uTP, `peer_proportional` (the default) limits
    // its tcp class to four times the TCP peers' share of the download rate (at least 30 kB/s);
    // once they are idle the class is unlimited again.
    let f = fixture("mixed");
    let mut seeds = vec![seeder(&f, tcp_only)];
    for _ in 0..5 {
        seeds.push(seeder(&f, |s| {
            utp_only(s);
            s.upload_rate_limit = 100_000;
        }));
    }
    let dir = temp_dir("leech");
    let leech = start_with(|s| assert_eq!(s.mixed_mode_algorithm, MixedModeAlgorithm::PeerProportional));
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    let h = leech.handle();
    for s in &seeds {
        h.torrent(id).connect_peer(loopback(s.2));
    }
    let tcp_limit = || h.get_peer_class(PeerClassId::TCP).unwrap().download_limit;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut limits = Vec::new();
    loop {
        limits.push(tcp_limit());
        let mut alerts = Vec::new();
        h.pop_alerts(&mut alerts);
        if alerts.iter().any(|a| matches!(a, Alert::TorrentFinished { id: i } if *i == id)) {
            break;
        }
        assert!(Instant::now() < deadline, "the download did not finish");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(limits.iter().any(|&l| l >= 30_000), "the tcp class was never limited: {limits:?}");
    // a finished torrent has no busy peers
    let deadline = Instant::now() + Duration::from_secs(10);
    while tcp_limit() != 0 {
        assert!(Instant::now() < deadline, "the tcp class stayed limited");
        std::thread::sleep(Duration::from_millis(50));
    }
    leech.shutdown();
    assert_files(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    for (s, dir, _) in seeds {
        s.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn prefer_tcp_leaves_the_tcp_class_unlimited() {
    let ses = start_with(|s| s.mixed_mode_algorithm = MixedModeAlgorithm::PreferTcp);
    let h = ses.handle();
    let mut info = h.get_peer_class(PeerClassId::TCP).unwrap();
    info.download_limit = 50_000;
    h.set_peer_class(PeerClassId::TCP, info);
    // the next tick resets it (libtorrent sets the tcp class's limits every tick)
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.get_peer_class(PeerClassId::TCP).unwrap().download_limit != 0 {
        assert!(Instant::now() < deadline, "prefer_tcp kept the tcp class limited");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn local_class_peers_are_unchoked_outside_the_unchoke_slots() {
    // libtorrent `ignore_unchoke_slots`: a seed with one unchoke slot unchokes both of its
    // leeches once loopback maps to the local class
    let f = fixture("slots");
    let dir = temp_dir("seed");
    write_files(&f, &dir);
    let seed = start_with(|s| s.unchoke_slots_limit = 1);
    let mut sw = Watch::new(seed.handle());
    let port = listen_port(&mut sw);
    let sid = add(&mut sw, &f, &dir);
    wait_state(&mut sw, sid, TorrentState::Seeding, 20);
    let mut filter = IpFilter::new();
    filter.add_rule(ip("127.0.0.0"), ip("127.255.255.255"), PeerClassId::LOCAL.mask());
    seed.handle().set_peer_class_filter(filter);

    let mut leeches = Vec::new();
    for _ in 0..2 {
        // slow, so neither finishes before the other is unchoked
        let l = start_with(|s| s.download_rate_limit = 50_000);
        let mut w = Watch::new(l.handle());
        let ldir = temp_dir("leech");
        let id = add(&mut w, &f, &ldir);
        wait_state(&mut w, id, TorrentState::Downloading, 10);
        l.handle().torrent(id).connect_peer(loopback(port));
        leeches.push((l, ldir));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        seed.handle().torrent(sid).post_peer_info();
        let peers = wait(&mut sw, 5, "peer info", |a| match a {
            Alert::PeerInfo { id, peers } if *id == sid => Some(peers.clone()),
            _ => None,
        });
        let unchoked = peers.iter().filter(|p| !p.flags.contains(PeerFlags::CHOKED)).count();
        if unchoked == 2 {
            break;
        }
        assert!(Instant::now() < deadline, "{unchoked} of {} peers unchoked", peers.len());
        std::thread::sleep(Duration::from_millis(100));
    }
    for (l, ldir) in leeches {
        l.shutdown();
        let _ = std::fs::remove_dir_all(ldir);
    }
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
