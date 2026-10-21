use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::*;
use crate::alert::{BlockReason, FileOperation, PortmapTransport, Protocol, ProxyOperation};
use crate::error::Error;
use crate::metainfo::WebSeedKind;
use crate::params::{
    AddTorrentParams, DownloadPriority, MoveFlags, QueueMove, RemoveFlags, TorrentFlags, TorrentSource,
};
use crate::session::SessionParams;
use crate::settings::{EncLevel, EncPolicy, ProxyType};
use crate::status::{
    AnnounceEntry, BlockState, FileEntry, PartialPiece, PeerFlags, PeerInfo, PeerSource, TorrentState,
    TrackerStatus,
};
use crate::types::{InfoHashes, Sha1Hash, Sha256Hash, TorrentId};
use crate::util::bitfield::Bitfield;

// --- the codec -------------------------------------------------------------------------------

fn encoded(a: &Alert) -> Vec<u8> {
    wire::to_bytes(|e| wire::encode_alert(a, e))
}

fn hashes() -> InfoHashes {
    InfoHashes { v1: Some(Sha1Hash([7; 20])), v2: Some(Sha256Hash([9; 32])) }
}

fn status() -> TorrentStatus {
    TorrentStatus {
        id: TorrentId::from_raw(0x1234_5678_9abc),
        info_hashes: hashes(),
        name: "ünïcode name".into(),
        save_path: r"D:\Torrents".into(),
        state: TorrentState::Seeding,
        flags: TorrentFlags::AUTO_MANAGED | TorrentFlags::SUPER_SEEDING,
        error: "disk full".into(),
        has_metadata: true,
        is_finished: true,
        is_seeding: true,
        super_seeding: true,
        moving_storage: false,
        progress_ppm: 1_000_000,
        queue_position: Some(3),
        download_rate: 1,
        upload_rate: 2,
        download_payload_rate: 3,
        upload_payload_rate: 4,
        num_seeds: 5,
        num_peers: 6,
        list_seeds: 7,
        list_peers: 8,
        num_complete: Some(9),
        num_incomplete: None,
        num_connections: 10,
        distributed_copies: Some(1.375),
        total_download: u64::MAX,
        total_upload: 1 << 40,
        total_payload_download: 11,
        total_payload_upload: 12,
        total_failed_bytes: 13,
        total_redundant_bytes: 14,
        all_time_download: 15,
        all_time_upload: 16,
        total: 17,
        total_done: 18,
        total_wanted: 19,
        total_wanted_done: 20,
        num_pieces: 21,
        pieces_total: 22,
        piece_length: 1 << 20,
        added_time: 1_792_490_437,
        completed_time: -1,
        last_seen_complete: 23,
        active_duration: 24,
        finished_duration: 25,
        seeding_duration: 26,
        current_tracker: "https://tracker.example/announce".into(),
        next_announce: Some(27),
        num_trackers: 28,
        announce_interval: None,
        announcing_to_trackers: true,
        announcing_to_lsd: false,
        announcing_to_dht: true,
        download_limit: 29,
        upload_limit: 30,
        connections_limit: Some(31),
        uploads_limit: None,
    }
}

fn every_alert() -> Vec<Alert> {
    let id = TorrentId::from_raw(42);
    let err = || Error::Io { kind: std::io::ErrorKind::PermissionDenied, message: "access denied".into() };
    let addr: SocketAddr = "[2001:db8::1]:6881".parse().unwrap();
    vec![
        Alert::TorrentAdded { id, info_hashes: hashes(), name: "a".into() },
        Alert::AddTorrentFailed { name: "b".into(), error: Error::DuplicateTorrent },
        Alert::TorrentRemoved { id, info_hashes: hashes() },
        Alert::TorrentDeleted { id, info_hashes: InfoHashes { v1: None, v2: Some(Sha256Hash([1; 32])) } },
        Alert::TorrentDeleteFailed { id, info_hashes: hashes(), error: err() },
        Alert::StorageMoved { id, old: "x".into(), new: "y".into() },
        Alert::StorageMovedFailed {
            id,
            error: err(),
            file: "f".into(),
            operation: FileOperation::PartfileMove,
        },
        Alert::FileRenamed { id, index: 3, old_name: "o".into(), new_name: "n".into() },
        Alert::FileRenameFailed { id, index: 4, error: err() },
        Alert::StateChanged { id, state: TorrentState::Finished, prev_state: TorrentState::Downloading },
        Alert::TorrentPaused { id },
        Alert::TorrentResumed { id },
        Alert::TorrentChecked { id },
        Alert::TorrentFinished { id },
        Alert::MetadataReceived { id },
        Alert::MetadataFailed { id, error: Error::MismatchingInfoHash },
        Alert::TorrentError { id, error: Error::SessionClosed },
        Alert::FileError { id, file: "g".into(), error: err() },
        Alert::TrackerReply { id, url: "u".into(), num_peers: 50 },
        Alert::TrackerWarning { id, url: "u".into(), message: "w".into() },
        Alert::TrackerError { id, url: "u".into(), times_in_row: 2, message: "e".into() },
        Alert::ScrapeReply { id, url: "u".into(), complete: 1, incomplete: 2 },
        Alert::ScrapeFailed { id, url: "u".into(), message: "m".into() },
        Alert::ListenSucceeded { address: addr, protocol: Protocol::Udp },
        Alert::ListenFailed { address: "eth0".into(), protocol: Protocol::Tcp, error: err() },
        Alert::PortmapSucceeded {
            transport: PortmapTransport::NatPmp,
            protocol: Protocol::Tcp,
            external_port: 6881,
        },
        Alert::PortmapFailed { transport: PortmapTransport::Upnp, message: "m".into() },
        Alert::PortmapLog { transport: PortmapTransport::Upnp, message: "l".into() },
        Alert::ExternalIp { ip: "203.0.113.5".parse().unwrap() },
        Alert::DhtReply { id, num_peers: 9 },
        Alert::DhtBootstrap,
        Alert::LsdPeer { id, address: addr },
        Alert::LsdError { address: "192.168.1.2".parse().unwrap(), error: err() },
        Alert::PeerBanned { id, addr, peer_id: *b"-PA0100-abcdefghijkl" },
        Alert::PeerBlocked { id: None, addr, reason: BlockReason::PortFilter },
        Alert::TorrentsRestored { count: 100_000, dir: "/var/lib/patorrent".into() },
        Alert::PeerDisconnected {
            id: Some(id),
            addr,
            peer_id: *b"-PA0100-abcdefghijkl",
            utp: true,
            reason: "both sides are seeds".into(),
            error: false,
        },
        Alert::Proxy { proxy: "p".into(), operation: ProxyOperation::Handshake, error: "e".into() },
        Alert::UrlSeed { id, url: "u".into(), message: "m".into() },
        Alert::StateUpdate(vec![status(), TorrentStatus::default()]),
        Alert::SessionStats(SessionStats {
            dht_nodes: 3,
            recv_bytes: 1 << 50,
            has_incoming_connections: true,
            ..Default::default()
        }),
        Alert::ResumeDataSaved,
        Alert::PeerInfo {
            id,
            peers: vec![PeerInfo {
                ip: addr,
                client: "qBittorrent 5".into(),
                flags: PeerFlags::SEED | PeerFlags::UTP_SOCKET,
                source: PeerSource::DHT | PeerSource::PEX,
                progress_ppm: 500_000,
                down_speed: 1,
                up_speed: 2,
                total_download: 3,
                total_upload: 4,
                download_queue_length: 5,
                upload_queue_length: 6,
                num_hashfails: 7,
                is_web_seed: false,
            }],
        },
        Alert::FileList {
            id,
            files: Arc::from(vec![FileEntry {
                path: "dir/file.bin".into(),
                size: 1 << 33,
                offset: 0,
                first_piece: 0,
                last_piece: 8191,
                pad_file: false,
                priority: DownloadPriority::TOP,
            }]),
        },
        Alert::FileProgress { id, done: vec![0, 1 << 33] },
        Alert::TrackerList {
            id,
            trackers: vec![AnnounceEntry {
                url: "udp://t:1".into(),
                tier: 1,
                status: TrackerStatus::Working,
                message: "ok".into(),
                scrape_complete: Some(1),
                scrape_incomplete: None,
                scrape_downloaded: Some(3),
                next_announce: Some(1800),
                fails: 2,
            }],
        },
        Alert::PieceInfo {
            id,
            have: Bitfield::from_bytes(&[0b1010_0000], 3).unwrap(),
            availability: vec![1, 0, 2],
            downloading: vec![PartialPiece {
                piece: 1,
                size: 3 * 16384,
                blocks: vec![
                    BlockState::Free,
                    BlockState::Requested(255),
                    BlockState::Writing,
                    BlockState::Finished,
                ],
            }],
        },
        Alert::WebSeeds { id, url_seeds: vec!["http://w/".into()], http_seeds: vec![] },
        Alert::TorrentFile { id, data: Some(Arc::from(&b"d4:infod4:name1:xee"[..])) },
        Alert::TorrentFile { id, data: None },
        Alert::Log { id: Some(id), message: "line".into() },
        Alert::AlertsDropped { count: 12 },
        Alert::RemoteDisconnected { message: "gone".into() },
    ]
}

#[test]
fn every_alert_round_trips() {
    for a in every_alert() {
        let back = wire::decode_alert(&encoded(&a)).unwrap_or_else(|| panic!("{a:?}"));
        assert_eq!(format!("{back:?}"), format!("{a:?}"));
    }
}

#[test]
fn errors_keep_their_identity_or_their_message() {
    let back = |e: Error| match wire::decode_alert(&encoded(&Alert::TorrentError {
        id: TorrentId::from_raw(1),
        error: e,
    })) {
        Some(Alert::TorrentError { error, .. }) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(back(Error::DuplicateTorrent), Error::DuplicateTorrent);
    let e = back(Error::Unsupported("x"));
    assert_eq!(e.to_string(), "not supported yet: x");
    let e = back(Error::InvalidResumeData("bad".into()));
    assert!(matches!(e, Error::Io { kind: std::io::ErrorKind::Other, .. }), "{e:?}");
}

#[test]
fn commands_round_trip() {
    let id = TorrentId::from_raw(7);
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(b"d4:infod4:name1:xee".to_vec()), "/srv/t");
    p.name = Some("n".into());
    p.trackers = vec!["udp://a:1".into(), "http://b/announce".into()];
    p.tracker_tiers = vec![0, 1];
    p.file_priorities = vec![DownloadPriority::LOW, DownloadPriority::DONT_DOWNLOAD];
    p.max_connections = Some(50);
    p.renamed_files = BTreeMap::from([(0, "a".into()), (5, "b/c".into())]);
    let settings = crate::settings::Settings {
        user_agent: "x".into(),
        download_rate_limit: 12_345,
        enable_dht: false,
        out_enc_policy: EncPolicy::Forced,
        allowed_enc_level: EncLevel::Rc4,
        proxy_type: ProxyType::Socks5Pw,
        proxy_port: 1080,
        ..Default::default()
    };
    let commands = vec![
        Command::AddTorrent(Box::new(p)),
        Command::AddTorrent(Box::new(AddTorrentParams::new(
            TorrentSource::Magnet("magnet:?xt=urn:btih:00".into()),
            "",
        ))),
        Command::AddTorrent(Box::new(AddTorrentParams::new(TorrentSource::InfoHash(hashes()), "/x"))),
        Command::AddTorrent(Box::new(AddTorrentParams::new(TorrentSource::Resume(vec![1, 2, 3]), "/y"))),
        Command::RemoveTorrent(id, RemoveFlags::DELETE_FILES | RemoveFlags::DELETE_PARTFILE),
        Command::Pause(id),
        Command::Resume(id),
        Command::SetFlags {
            id,
            flags: TorrentFlags::PAUSED,
            mask: TorrentFlags::PAUSED | TorrentFlags::AUTO_MANAGED,
        },
        Command::Queue(id, QueueMove::Bottom),
        Command::ForceRecheck(id),
        Command::ForceReannounce(id),
        Command::ScrapeTracker(id, Some(2)),
        Command::ScrapeTracker(id, None),
        Command::ScrapeTrackers(vec![id, TorrentId::from_raw(8)]),
        Command::SetFilePriority { id, file: 3, priority: DownloadPriority::TOP },
        Command::PrioritizeFiles(id, vec![DownloadPriority::NORMAL]),
        Command::SetDownloadLimit(id, 1),
        Command::SetUploadLimit(id, 2),
        Command::SetMaxConnections(id, 3),
        Command::SetMaxUploads(id, 4),
        Command::ReplaceTrackers(id, vec![AnnounceEntry::new("http://t/a", 0)]),
        Command::AddTracker(id, AnnounceEntry::new("udp://t:2", 3)),
        Command::MoveStorage(id, "/new".into(), MoveFlags::DontReplace),
        Command::RenameFile(id, 1, "x/y".into()),
        Command::ConnectPeer(id, "10.0.0.1:6881".parse().unwrap()),
        Command::AddWebSeed(id, "http://w/".into(), WebSeedKind::HttpSeed),
        Command::RemoveWebSeed(id, "http://w/".into(), WebSeedKind::UrlSeed),
        Command::PostWebSeeds(id),
        Command::PostTorrentUpdates,
        Command::PostSessionStats,
        Command::SaveResumeData,
        Command::PostPeerInfo(id),
        Command::PostFileList(id),
        Command::PostFileProgress(id),
        Command::PostTrackers(id),
        Command::PostPieceInfo(id),
        Command::PostTorrentFile(id),
        Command::ApplySettings(Box::new(settings)),
        Command::PauseSession,
        Command::ResumeSession,
        Command::ReopenNetworkSockets,
    ];
    for c in commands {
        let mut e = wire::Enc::default();
        assert!(wire::encode_command(&c, &mut e), "{c:?}");
        let back = wire::decode_command(&e.0).unwrap_or_else(|| panic!("{c:?}"));
        assert_eq!(format!("{back:?}"), format!("{c:?}"));
    }
    // these stay on the client
    for c in [Command::Shutdown, Command::SetPortFilter(Default::default())] {
        assert!(!wire::encode_command(&c, &mut wire::Enc::default()));
    }
}

#[test]
fn the_hello_carries_the_settings() {
    let hello = Hello {
        server_version: "0.1.0".into(),
        save_path: "/srv/torrents".into(),
        settings: crate::settings::Settings { active_downloads: 9, ..Default::default() },
    };
    let mut out = Vec::new();
    hello_frame(&hello, &mut out);
    let mut r = FrameReader::new(out, MAX_FRAME);
    assert_eq!(decode_hello(&r.next_frame().unwrap().unwrap()), Some((PROTOCOL, hello)));
    assert_eq!(r.next_frame().unwrap(), None);
}

#[test]
fn malformed_frames_are_refused() {
    let good =
        encoded(&Alert::TorrentAdded { id: TorrentId::from_raw(1), info_hashes: hashes(), name: "n".into() });
    // every truncation, a wrong type, an unknown message, too many elements for an Option
    for n in 0..good.len() {
        assert!(wire::decode_alert(&good[..n]).is_none(), "{n}");
    }
    assert!(wire::decode_alert(b"l12:TorrentAdded3:abc20:xxxxxxxxxxxxxxxxxxxx1:ne").is_none());
    assert!(wire::decode_alert(b"l7:Unknowne").is_none());
    assert!(wire::decode_alert(b"l3:Logli1ei2ee1:me").is_none());
    assert!(wire::decode_command(b"l5:pause3:xyze").is_none());
    assert!(wire::decode_command(b"i5e").is_none());
    // frames: one split in two reads, one too big
    let mut out = Vec::new();
    alert_frame(&Alert::DhtBootstrap, &mut out);
    let (a, b) = out.split_at(5);
    let mut r = FrameReader::new(a.to_vec(), MAX_FRAME);
    assert_eq!(r.next_frame().unwrap(), None);
    r.buf.extend_from_slice(b);
    assert!(matches!(wire::decode_alert(&r.next_frame().unwrap().unwrap()), Some(Alert::DhtBootstrap)));
    let mut r = FrameReader::new(vec![0, 0, 1, 0, 0], 255);
    assert!(r.next_frame().is_err());
}

// --- over loopback ---------------------------------------------------------------------------

const USER: &str = "admin";
const PASS: &str = "pw";

fn engine() -> Session {
    let settings = crate::settings::Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        tick_interval: 100,
        ..Default::default()
    };
    Session::start(SessionParams { settings, state_dir: None }).unwrap()
}

trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

/// How the test server behaves.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Serve,
    /// Close the connection after the hello.
    HangUp,
    /// Answer the upgrade with this status.
    Refuse(u16),
}

/// A minimal server: one connection, the upgrade checked as nox checks it (path, upgrade
/// token, version, Basic login), then the engine's alerts out and the client's commands in.
fn serve(
    engine: SessionHandle,
    mode: Mode,
    tls: Option<crate::tls::Identity>,
) -> (u16, Arc<AtomicBool>, JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let done = Arc::new(AtomicBool::new(false));
    let stop = done.clone();
    let t = std::thread::spawn(move || {
        let (sock, _) = l.accept().unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let mut s: Box<dyn Stream> = match &tls {
            Some(id) => {
                sock.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut t = crate::tls::Stream::accept(sock.try_clone().unwrap(), id).unwrap();
                // a probe ends the connection right after its handshake
                if t.handshake().is_err() {
                    return;
                }
                // (on Windows a cloned socket has timeouts of its own)
                t.get_ref().set_read_timeout(Some(Duration::from_millis(20))).unwrap();
                Box::new(t)
            }
            None => Box::new(sock.try_clone().unwrap()),
        };
        let mut r = FrameReader::new(Vec::new(), MAX_FRAME);
        let end = loop {
            if let Some(p) = r.buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break p + 4;
            }
            match r.read_from(&mut s) {
                Ok(true) => {}
                // (the end of a probe's connection)
                Ok(false) => return,
                Err(e) if is_timeout(&e) => {}
                Err(_) => return,
            }
        };
        let head = String::from_utf8_lossy(&r.buf[..end]).into_owned();
        r.buf.drain(..end);
        let auth =
            format!("Basic {}", crate::util::string::base64encode(format!("{USER}:{PASS}").as_bytes()));
        let status = match mode {
            Mode::Refuse(s) => s,
            _ if !head.starts_with(&format!("GET {PATH} HTTP/1.1\r\n")) => 404,
            _ if !head.contains(&format!("Upgrade: {UPGRADE}\r\n")) => 400,
            _ if !head.contains(&format!("{VERSION_HEADER}: {PROTOCOL}\r\n")) => 426,
            _ if !head.contains(&format!("Authorization: {auth}\r\n")) => 401,
            _ => 101,
        };
        if status != 101 {
            let body = "go away";
            let _ = write!(s, "HTTP/1.1 {status} No\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            let _ = s.flush();
            return;
        }
        let mut out =
            format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: {UPGRADE}\r\n\r\n")
                .into_bytes();
        hello_frame(
            &Hello { server_version: "test".into(), save_path: "/srv".into(), settings: Default::default() },
            &mut out,
        );
        s.write_all(&out).unwrap();
        s.flush().unwrap();
        if mode == Mode::HangUp {
            let _ = sock.shutdown(std::net::Shutdown::Both);
            return;
        }
        let mut alerts = Vec::new();
        while !stop.load(Ordering::SeqCst) {
            match r.read_from(&mut s) {
                Ok(true) => {}
                Ok(false) => return,
                Err(e) if is_timeout(&e) => {}
                Err(_) => return,
            }
            while let Some(frame) = r.next_frame().unwrap() {
                match dispatch(&engine, &frame).expect("a command") {
                    Request::Forwarded => {}
                    Request::PostTorrentUpdates => engine.post_torrent_updates(),
                    Request::PostSessionStats => engine.post_session_stats(),
                    Request::ApplySettings(st) => engine.apply_settings(*st),
                }
            }
            engine.pop_alerts(&mut alerts);
            let mut out = Vec::new();
            for a in &alerts {
                alert_frame(a, &mut out);
            }
            if !out.is_empty() && (s.write_all(&out).is_err() || s.flush().is_err()) {
                return;
            }
        }
    });
    (port, done, t)
}

fn target(port: u16, tls: Option<Verify>) -> Target {
    Target { host: "127.0.0.1".into(), port, tls, username: USER.into(), password: PASS.into() }
}

/// Pops the client's alerts until `f` finds what it wants.
fn wait_for<T>(h: &SessionHandle, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut alerts = Vec::new();
    while Instant::now() < deadline {
        h.pop_alerts(&mut alerts);
        if let Some(t) = alerts.iter().find_map(&mut f) {
            return t;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out");
}

fn drive(client: &Session) {
    let h = client.handle();
    let magnet = format!("magnet:?xt=urn:btih:{}&dn=remote-test", "ab".repeat(20));
    h.add_torrent(AddTorrentParams::new(
        TorrentSource::Magnet(magnet),
        std::env::temp_dir().to_string_lossy(),
    ));
    let id = wait_for(&h, |a| match a {
        Alert::TorrentAdded { id, name, .. } if name == "remote-test" => Some(*id),
        _ => None,
    });
    h.post_torrent_updates();
    let st = wait_for(&h, |a| match a {
        Alert::StateUpdate(v) => v.iter().find(|s| s.id == id).cloned(),
        _ => None,
    });
    assert_eq!(st.state, TorrentState::DownloadingMetadata);
    assert_eq!(st.info_hashes.v1, Some(Sha1Hash([0xab; 20])));
    h.post_session_stats();
    let stats = wait_for(&h, |a| match a {
        Alert::SessionStats(s) => Some(*s),
        _ => None,
    });
    assert_eq!(stats.num_torrents, 1);
    h.torrent(id).post_trackers();
    wait_for(&h, |a| matches!(a, Alert::TrackerList { id: x, .. } if *x == id).then_some(()));
    h.remove_torrent(id, RemoveFlags::empty());
    wait_for(&h, |a| matches!(a, Alert::TorrentRemoved { id: x, .. } if *x == id).then_some(()));
    // a filter table stays here, said in the log
    h.set_port_filter(Default::default());
    wait_for(&h, |a| matches!(a, Alert::Log { message, .. } if message.contains("IP filter")).then_some(()));
}

#[test]
fn a_remote_session_drives_the_engine() {
    let server_engine = engine();
    let (port, done, server) = serve(server_engine.handle(), Mode::Serve, None);
    let (client, hello) = connect(&target(port, None), Duration::from_secs(10)).unwrap();
    assert_eq!((hello.server_version.as_str(), hello.save_path.as_str()), ("test", "/srv"));
    drive(&client);
    client.shutdown();
    server.join().unwrap();
    done.store(true, Ordering::SeqCst);
}

#[test]
fn a_remote_session_over_tls() {
    let pem = crate::tls::generate_self_signed(&["localhost"], 1).unwrap();
    let id = crate::tls::Identity::from_pem(pem.cert_pem.as_bytes(), pem.key_pem.as_bytes()).unwrap();
    let pin = id.fingerprint();
    let server_engine = engine();
    let (port, done, server) = serve(server_engine.handle(), Mode::Serve, Some(id.clone()));
    // the user is shown the fingerprint first; nothing reaches the server's HTTP side
    assert_eq!(probe_certificate("127.0.0.1", port, Duration::from_secs(10)).unwrap(), pin);
    // (the probe used the test server's one connection)
    server.join().unwrap();
    let (port, _, server) = serve(server_engine.handle(), Mode::Serve, Some(id));
    let (client, _) = connect(&target(port, Some(Verify::Pinned(pin))), Duration::from_secs(10)).unwrap();
    drive(&client);
    drop(client);
    server.join().unwrap();
    done.store(true, Ordering::SeqCst);
}

#[test]
fn a_lost_connection_is_reported_once() {
    let server_engine = engine();
    let (port, _, server) = serve(server_engine.handle(), Mode::HangUp, None);
    let (client, _) = connect(&target(port, None), Duration::from_secs(10)).unwrap();
    let h = client.handle();
    let message = wait_for(&h, |a| match a {
        Alert::RemoteDisconnected { message } => Some(message.clone()),
        _ => None,
    });
    assert!(!message.is_empty());
    // commands go nowhere and nothing else arrives; the shutdown still ends the thread
    h.post_torrent_updates();
    std::thread::sleep(Duration::from_millis(100));
    let mut rest = Vec::new();
    h.pop_alerts(&mut rest);
    assert!(rest.is_empty(), "{rest:?}");
    client.shutdown();
    server.join().unwrap();
}

#[test]
fn refusals_say_why() {
    let server_engine = engine();
    for (status, want) in [(401, "wrong user name or password"), (426, "HTTP 426): go away")] {
        let (port, _, server) = serve(server_engine.handle(), Mode::Refuse(status), None);
        let e = connect(&target(port, None), Duration::from_secs(10)).err().unwrap();
        assert!(matches!(e, ConnectError::Refused { status: s, .. } if s == status), "{e:?}");
        assert!(e.to_string().contains(want), "{e}");
        server.join().unwrap();
    }
    // a wrong password, checked by the test server like the others
    let (port, _, server) = serve(server_engine.handle(), Mode::Serve, None);
    let mut t = target(port, None);
    t.password = "nope".into();
    assert!(matches!(connect(&t, Duration::from_secs(10)), Err(ConnectError::Refused { status: 401, .. })));
    server.join().unwrap();
    // nothing listening
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert!(matches!(connect(&target(port, None), Duration::from_secs(2)), Err(ConnectError::Io(_))));
    // a plain HTTP server that is not ours
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let t = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut b = [0u8; 1024];
        let _ = s.read(&mut b);
        let _ = s.write_all(b"SSH-2.0-OpenSSH_9.9\r\n");
    });
    let e = connect(&target(port, None), Duration::from_secs(5)).err().unwrap();
    assert!(matches!(e, ConnectError::Protocol(_)), "{e:?}");
    t.join().unwrap();
    let _ = TcpStream::connect(("127.0.0.1", 1));
}
