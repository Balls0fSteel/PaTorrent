//! File > Import end to end: a µTorrent `resume.dat` and a qBittorrent `BT_backup` turned into
//! resume records and added to a real session. The engine trusts the pieces the source claims
//! when their files are there (no full check), finds the data under a folder µTorrent renamed,
//! and checks the files instead when they are missing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::metainfo::TorrentInfo;
use patorrent_core::{Alert, Session, SessionParams, Settings, TorrentState, TorrentStatus};
use patorrent_frontend::import::{self, Client};

const PIECE: usize = 16 * 1024;

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("patorrent-import-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A torrent `Show` of two files, 5 pieces in all, and the files' contents.
fn fixture() -> (Vec<u8>, Vec<(&'static str, Vec<u8>)>) {
    let mut x = 0x2545_f491u32;
    let mut data = |n: usize| -> Vec<u8> {
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    };
    let files = vec![("a.bin", data(40_000)), ("b.bin", data(30_000))];
    let all: Vec<u8> = files.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    let pieces: Vec<u8> = all.chunks(PIECE).flat_map(|c| Sha1::digest(c).0).collect();
    let mut info = Entry::new_dict();
    *info.key("files") = Entry::List(
        files
            .iter()
            .map(|(name, d)| {
                let mut f = Entry::new_dict();
                *f.key("length") = Entry::Int(d.len() as i64);
                *f.key("path") = Entry::List(vec![Entry::Bytes(name.as_bytes().to_vec())]);
                f
            })
            .collect(),
    );
    *info.key("name") = Entry::Bytes(b"Show".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    *t.key("comment") = Entry::Bytes(b"from the test".to_vec());
    (t.bencode(), files)
}

fn session() -> Session {
    let settings = Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        tick_interval: 100,
        aio_threads: 2,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    };
    Session::start(SessionParams { settings, state_dir: None }).expect("session starts")
}

/// Adds the import: its first status (as the source left it), then starts it (a stopped
/// torrent checks its resume data only when started, as in libtorrent) and returns its status
/// once that check is over.
fn add_and_settle(s: &Session, t: import::Import) -> (TorrentStatus, TorrentStatus) {
    let h = s.handle();
    h.add_torrent(t.params);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut alerts = Vec::new();
    let mut first: Option<TorrentStatus> = None;
    let mut last = None;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        h.post_torrent_updates();
        h.pop_alerts(&mut alerts);
        for a in alerts.drain(..) {
            match a {
                Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
                Alert::StateUpdate(v) => {
                    for st in v {
                        let Some(f) = &first else {
                            patorrent_frontend::actions::start(&h.torrent(st.id));
                            first = Some(st);
                            continue;
                        };
                        let checking = matches!(
                            st.state,
                            TorrentState::CheckingResumeData | TorrentState::CheckingFiles
                        );
                        if !checking {
                            return (f.clone(), st);
                        }
                        last = Some(st);
                    }
                }
                _ => {}
            }
        }
    }
    panic!("not settled: {last:?}");
}

/// µTorrent's `resume.dat` with one torrent at `path`, claiming the pieces in `have`.
fn utorrent_dir(name: &str, torrent: &[u8], path: &Path, have: &[usize]) -> PathBuf {
    let dir = temp_dir(name);
    std::fs::write(dir.join("Show.torrent"), torrent).unwrap();
    let mut bits = vec![0u8; 1];
    for &p in have {
        bits[p / 8] |= 1 << (p % 8);
    }
    let mut rec = Entry::new_dict();
    *rec.key("caption") = Entry::Bytes(b"My Show".to_vec());
    *rec.key("path") = Entry::Bytes(path.to_string_lossy().as_bytes().to_vec());
    *rec.key("have") = Entry::Bytes(bits);
    *rec.key("prio") = Entry::Bytes(vec![8, 8]);
    *rec.key("started") = Entry::Int(0);
    *rec.key("uploaded") = Entry::Int(777);
    let mut root = Entry::new_dict();
    *root.key("Show.torrent") = rec;
    std::fs::write(dir.join("resume.dat"), root.bencode()).unwrap();
    dir
}

#[test]
fn utorrent_pieces_are_trusted_under_a_renamed_folder() {
    let (torrent, files) = fixture();
    let data = temp_dir("ut-data");
    // µTorrent moved the data into a folder of another name
    let content = data.join("Show (renamed)");
    std::fs::create_dir_all(&content).unwrap();
    for (name, d) in &files {
        std::fs::write(content.join(name), d).unwrap();
    }
    // the files are complete, but µTorrent only claims pieces 0, 1 and 4: a full check would
    // find all five, so three shows the claim was trusted
    let dir = utorrent_dir("ut", &torrent, &content, &[0, 1, 4]);
    let mut found = import::read(Client::UTorrent, &dir).unwrap();
    assert!(found.skipped.is_empty(), "{:?}", found.skipped);
    let mut prefs = patorrent_frontend::prefs::Prefs::from_text("");
    let mut add = import::prepare(&mut found, &mut prefs, |_| false);
    let s = session();
    let (first, st) = add_and_settle(&s, add.remove(0));
    assert!(first.flags.contains(patorrent_core::TorrentFlags::PAUSED), "µTorrent's Stopped");
    assert_eq!(st.name, "My Show");
    assert_eq!(st.num_pieces, 3, "the claimed pieces, without a full check");
    assert_eq!(st.all_time_upload, 777);
    assert_eq!(st.save_path, data.to_string_lossy());
    // the imported data stays as it was
    for (name, d) in &files {
        assert_eq!(&std::fs::read(content.join(name)).unwrap(), d);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn missing_files_mean_a_full_check() {
    let (torrent, _) = fixture();
    let data = temp_dir("ut-missing");
    let dir = utorrent_dir("ut-missing-src", &torrent, &data.join("Show"), &[0, 1, 2, 3, 4]);
    let mut found = import::read(Client::UTorrent, &dir).unwrap();
    let mut prefs = patorrent_frontend::prefs::Prefs::from_text("");
    let mut add = import::prepare(&mut found, &mut prefs, |_| false);
    let s = session();
    let (first, st) = add_and_settle(&s, add.remove(0));
    assert_eq!(first.num_pieces, 5, "claimed until started");
    assert_eq!(st.num_pieces, 0, "nothing claimed without its files");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn qbittorrent_record_is_taken_as_it_is() {
    let (torrent, files) = fixture();
    let data = temp_dir("qbt-data");
    std::fs::create_dir_all(data.join("Show")).unwrap();
    for (name, d) in &files {
        std::fs::write(data.join("Show").join(name), d).unwrap();
    }
    let ti = TorrentInfo::from_bytes(&torrent).unwrap();
    let hex = ti.info_hashes().key_hex();
    let dir = temp_dir("qbt").join("BT_backup");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{hex}.torrent")), &torrent).unwrap();
    let mut rec = Entry::new_dict();
    *rec.key("file-format") = Entry::Bytes(b"libtorrent resume file".to_vec());
    *rec.key("file-version") = Entry::Int(1);
    *rec.key("info-hash") = Entry::Bytes(ti.info_hash().0.to_vec());
    *rec.key("save_path") = Entry::Bytes(data.to_string_lossy().as_bytes().to_vec());
    *rec.key("pieces") = Entry::Bytes(vec![1, 1, 1, 0, 1]);
    *rec.key("paused") = Entry::Int(1);
    *rec.key("auto_managed") = Entry::Int(0);
    *rec.key("total_uploaded") = Entry::Int(4242);
    *rec.key("qBt-category") = Entry::Bytes(b"tv".to_vec());
    std::fs::write(dir.join(format!("{hex}.fastresume")), rec.bencode()).unwrap();

    let mut found = import::read(Client::QBittorrent, &dir).unwrap();
    let mut prefs = patorrent_frontend::prefs::Prefs::from_text("");
    let mut add = import::prepare(&mut found, &mut prefs, |_| false);
    assert_eq!(prefs.get(&format!("torrent.{hex}.label")), Some("tv"));
    let s = session();
    let (_, st) = add_and_settle(&s, add.remove(0));
    assert_eq!((st.name.as_str(), st.num_pieces, st.all_time_upload), ("Show", 4, 4242));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    let _ = std::fs::remove_dir_all(&data);
}
