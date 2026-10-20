//! The "Create New Torrent" dialog's logic, independent of the toolkit: parsing its fields and
//! building the `.torrent` (on whatever thread the caller runs it).

use std::path::{Path, PathBuf};

use patorrent_core::metainfo::create_torrent::{get_file_attributes, parent_dir};
use patorrent_core::metainfo::{
    CreateFlags, CreateTorrent, CreateTorrentError, FileStorage, TorrentInfo, add_files, set_piece_hashes,
};
use patorrent_core::{AddTorrentParams, Sha1Hash, TorrentFlags, TorrentSource, VERSION};

use crate::lang::S;
use crate::tr;

/// The piece size choices of µTorrent 2.2.1 in bytes: 0 for "(auto detect)", then 16 KiB to
/// 16 MiB.
pub const PIECE_SIZES: [u32; 12] = [
    0,
    16 << 10,
    32 << 10,
    64 << 10,
    128 << 10,
    256 << 10,
    512 << 10,
    1 << 20,
    2 << 20,
    4 << 20,
    8 << 20,
    16 << 20,
];

/// A piece size choice's text: `(auto detect)`, `16kB`, `1MB`.
pub fn piece_size_text(bytes: u32) -> String {
    match bytes {
        0 => tr!(CreateAutoDetect).to_owned(),
        b if b >= 1 << 20 => format!("{}{}", b >> 20, tr!(UnitMb)),
        b => format!("{}{}", b >> 10, tr!(UnitKb)),
    }
}

/// The BitTorrent version of a new torrent (BEP 52).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TorrentFormat {
    /// v2 plus v1: loads in v1-only and v2 clients alike. The default, as in libtorrent and
    /// qBittorrent.
    #[default]
    Hybrid,
    /// v1 only, as µTorrent 2.2.1 made them.
    V1,
    /// v2 only: v1-only clients cannot load it.
    V2,
}

/// The "Version" choices, in list order.
pub const FORMATS: [(S, TorrentFormat); 3] = [
    (S::CreateHybrid, TorrentFormat::Hybrid),
    (S::CreateV1, TorrentFormat::V1),
    (S::CreateV2Only, TorrentFormat::V2),
];

impl TorrentFormat {
    /// The creation flags for this version.
    pub fn flags(self) -> CreateFlags {
        match self {
            TorrentFormat::Hybrid => CreateFlags::empty(),
            TorrentFormat::V1 => CreateFlags::V1_ONLY,
            TorrentFormat::V2 => CreateFlags::V2_ONLY,
        }
    }

    /// The preferences value.
    pub fn key(self) -> &'static str {
        match self {
            TorrentFormat::Hybrid => "hybrid",
            TorrentFormat::V1 => "v1",
            TorrentFormat::V2 => "v2",
        }
    }

    pub fn from_key(s: &str) -> Option<TorrentFormat> {
        [TorrentFormat::Hybrid, TorrentFormat::V1, TorrentFormat::V2].into_iter().find(|f| f.key() == s)
    }

    /// Position in [`FORMATS`].
    pub fn index(self) -> usize {
        FORMATS.iter().position(|(_, f)| *f == self).expect("listed")
    }

    /// Whether the files may keep the directory's listing order ("Preserve file order"): v2
    /// lays them out sorted.
    pub fn allows_any_order(self) -> bool {
        self == TorrentFormat::V1
    }
}

/// What the dialog collected.
#[derive(Clone, Debug, Default)]
pub struct CreateOptions {
    /// A file or a directory.
    pub source: PathBuf,
    /// The "Skip Files" field (see [`SkipPatterns`]).
    pub skip: String,
    /// The Trackers box: one URL per line, a blank line between tiers.
    pub trackers: String,
    /// The Web Seeds box: one URL per line.
    pub web_seeds: String,
    pub comment: String,
    /// Bytes; 0 picks one from the total size.
    pub piece_size: u32,
    pub private: bool,
    /// Keep the files in the order the directory lists them instead of sorting them by name
    /// (v1 only; v2 and hybrid torrents are always sorted).
    pub preserve_order: bool,
    pub format: TorrentFormat,
}

/// A finished torrent.
#[derive(Debug)]
pub struct Created {
    pub bytes: Vec<u8>,
    pub name: String,
    pub info_hash: Sha1Hash,
}

/// Trackers by tier: lines are URLs, blank lines separate tiers (µTorrent's Trackers box).
pub fn parse_trackers(text: &str) -> Vec<Vec<String>> {
    let mut tiers: Vec<Vec<String>> = vec![Vec::new()];
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            if !tiers.last().is_some_and(Vec::is_empty) {
                tiers.push(Vec::new());
            }
        } else if !tiers.iter().flatten().any(|u| u == line) {
            tiers.last_mut().unwrap().push(line.to_owned());
        }
    }
    tiers.retain(|t| !t.is_empty());
    tiers
}

/// Non-empty trimmed lines, duplicates dropped.
pub fn parse_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !out.iter().any(|u| u == line) {
            out.push(line.to_owned());
        }
    }
    out
}

/// The "Skip Files" field: wildcard patterns (`*`, `?`) separated by `|` or `;`, matched without
/// case against the name of each file and folder below the source.
#[derive(Clone, Debug, Default)]
pub struct SkipPatterns(Vec<Vec<char>>);

impl SkipPatterns {
    pub fn parse(text: &str) -> SkipPatterns {
        SkipPatterns(
            text.split(['|', ';'])
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| p.to_lowercase().chars().collect())
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn matches(&self, name: &str) -> bool {
        let name: Vec<char> = name.to_lowercase().chars().collect();
        self.0.iter().any(|p| wildcard(p, &name))
    }
}

/// `*` matches any run (also empty), `?` one character.
fn wildcard(p: &[char], s: &[char]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// The Trackers box as one preferences line: URLs separated by spaces, tiers by ` | `.
pub fn trackers_to_pref(text: &str) -> String {
    parse_trackers(text).iter().map(|t| t.join(" ")).collect::<Vec<_>>().join(" | ")
}

/// [`trackers_to_pref`] back to the Trackers box (CRLF lines, a blank line between tiers).
pub fn trackers_from_pref(s: &str) -> String {
    s.split('|')
        .map(|tier| tier.split_whitespace().collect::<Vec<_>>().join("\r\n"))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\r\n\r\n")
}

/// The file name the save dialog suggests: the source's name plus `.torrent`.
pub fn default_file_name(source: &Path) -> String {
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| std::path::absolute(source).ok()?.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "new".into());
    format!("{name}.torrent")
}

/// Where a torrent made from `source` is seeded from: the folder that holds it.
pub fn seed_save_path(source: &Path) -> std::io::Result<PathBuf> {
    parent_dir(source)
}

/// "Start seeding": adds the new torrent with `save_path` (from [`seed_save_path`]) as its
/// download folder, queued and auto-managed. The engine checks the files and then seeds.
pub fn seed_params(bytes: Vec<u8>, save_path: &Path) -> AddTorrentParams {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes), save_path.to_string_lossy());
    p.flags = TorrentFlags::DEFAULT;
    p
}

/// Lists the files, hashes them and returns the bencoded torrent. `progress(done, total)` is
/// called after each piece and returns false to cancel (the result is then `Err("cancelled")`).
pub fn build(o: &CreateOptions, progress: &mut dyn FnMut(u32, u32) -> bool) -> Result<Created, String> {
    let source = &o.source;
    if source.as_os_str().is_empty() {
        return Err(tr!(CreateNoSource).into());
    }
    let meta = std::fs::metadata(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let skip = SkipPatterns::parse(&o.skip);
    let root = std::path::absolute(source).map_err(|e| e.to_string())?;
    let mut fs = FileStorage::new();
    if o.preserve_order && o.format.allows_any_order() && meta.is_dir() {
        let name = root.file_name().and_then(|n| n.to_str()).ok_or(tr!(CreateNoName))?;
        add_unsorted(&mut fs, &root, name, &skip)?;
    } else {
        let mut pred =
            |p: &Path| p == root || !p.file_name().is_some_and(|n| skip.matches(&n.to_string_lossy()));
        add_files(&mut fs, &root, &mut pred, CreateFlags::empty()).map_err(|e| e.to_string())?;
    }
    if fs.num_files() == 0 || fs.total_size() == 0 {
        return Err(tr!(CreateNoFiles, path = source.display()));
    }

    let mut t = CreateTorrent::new(fs, o.piece_size, o.format.flags()).map_err(|e| e.to_string())?;
    for (tier, urls) in parse_trackers(&o.trackers).iter().enumerate() {
        for url in urls {
            t.add_tracker(url, tier as u32);
        }
    }
    for url in parse_lines(&o.web_seeds) {
        t.add_url_seed(&url);
    }
    t.set_comment(o.comment.trim());
    t.set_priv(o.private);
    t.set_creator(&format!("PaTorrent/{VERSION}"));

    let base = parent_dir(&root).map_err(|e| e.to_string())?;
    let total = t.num_pieces();
    let mut cancelled = false;
    let hashed = set_piece_hashes(&mut t, &base, &mut |done| {
        let go = progress(done + 1, total);
        cancelled |= !go;
        go
    });
    if cancelled {
        return Err("cancelled".into());
    }
    hashed.map_err(|e| e.to_string())?;

    let bytes = t.generate_buf().map_err(|e| e.to_string())?;
    let ti = TorrentInfo::from_bytes(&bytes).map_err(|e| tr!(CreateDoesNotLoad, error = e))?;
    Ok(Created { name: ti.name().to_owned(), info_hash: ti.info_hash(), bytes })
}

/// [`add_files`] keeping each directory's listing order (NTFS lists names case-insensitively,
/// [`add_files`] sorts them by code point).
fn add_unsorted(fs: &mut FileStorage, dir: &Path, rel: &str, skip: &SkipPatterns) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for e in entries.filter_map(Result::ok) {
        let name = e.file_name();
        let Some(name) = name.to_str() else {
            return Err(tr!(CreateNotUnicode, path = e.path().display()));
        };
        if skip.matches(name) {
            continue;
        }
        let path = e.path();
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let rel = format!("{rel}/{name}");
        if meta.is_dir() {
            add_unsorted(fs, &path, &rel, skip)?;
        } else {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            fs.add_file(&rel, meta.len(), get_file_attributes(&path), mtime, "", None)
                .map_err(|e| CreateTorrentError::from(e).to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let p = std::env::temp_dir().join(format!("patorrent-fe-create-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The files, pad files left out.
    fn file_paths(bytes: &[u8]) -> Vec<String> {
        let ti = TorrentInfo::from_bytes(bytes).unwrap();
        let fs = ti.files();
        (0..fs.num_files())
            .filter(|&i| !fs.pad_file_at(i))
            .map(|i| fs.file_path(i, "").replace('\\', "/"))
            .collect()
    }

    #[test]
    fn tracker_tiers() {
        let t = parse_trackers("http://a/announce\r\n udp://b:80 \n\n\nhttp://c/\nhttp://a/announce\n\n");
        assert_eq!(
            t,
            vec![vec!["http://a/announce".to_owned(), "udp://b:80".into()], vec!["http://c/".into()]]
        );
        assert!(parse_trackers("\n \n").is_empty());
        assert_eq!(parse_lines(" http://w/ \n\nhttp://w/\nhttp://x/"), vec!["http://w/", "http://x/"]);
        let pref = trackers_to_pref("http://a/\nhttp://b/\n\n\nudp://c:1\n");
        assert_eq!(pref, "http://a/ http://b/ | udp://c:1");
        assert_eq!(trackers_from_pref(&pref), "http://a/\r\nhttp://b/\r\n\r\nudp://c:1");
        assert_eq!(trackers_from_pref(""), "");
    }

    #[test]
    fn skip_patterns() {
        let s = SkipPatterns::parse("*.TXT| thumbs.db ;a?c*");
        assert!(s.matches("readme.txt"));
        assert!(s.matches("Thumbs.db"));
        assert!(s.matches("abc"));
        assert!(s.matches("axcdef"));
        assert!(!s.matches("ac"));
        assert!(!s.matches("readme.txt.bak"));
        assert!(SkipPatterns::parse(" | ").is_empty());
        assert!(SkipPatterns::parse("*").matches(""));
    }

    #[test]
    fn piece_sizes_are_valid() {
        assert_eq!(PIECE_SIZES[0], 0);
        assert!(PIECE_SIZES[1..].iter().all(|&s| s.is_power_of_two() && s % (16 << 10) == 0));
        assert_eq!(*PIECE_SIZES.last().unwrap(), 16 << 20);
        let texts: Vec<String> = PIECE_SIZES.iter().map(|&s| piece_size_text(s)).collect();
        assert_eq!(texts[..3], ["(auto detect)", "16kB", "32kB"]);
        assert_eq!(texts[7..9], ["1MB", "2MB"]);
    }

    #[test]
    fn names() {
        assert_eq!(default_file_name(Path::new("C:/data/My Folder")), "My Folder.torrent");
        assert_eq!(default_file_name(Path::new("x/file.bin")), "file.bin.torrent");
        let d = TempDir::new("names");
        assert_eq!(seed_save_path(&d.0.join("sub")).unwrap(), d.0);
    }

    #[test]
    fn builds_a_folder_with_all_fields() {
        let d = TempDir::new("build");
        let src = d.0.join("Album");
        std::fs::create_dir_all(src.join("disc 1")).unwrap();
        std::fs::write(src.join("disc 1/01.flac"), vec![7u8; 100_000]).unwrap();
        std::fs::write(src.join("cover.jpg"), vec![1u8; 5_000]).unwrap();
        std::fs::write(src.join("notes.TXT"), b"skip me").unwrap();
        let o = CreateOptions {
            source: src.clone(),
            skip: "*.txt".into(),
            trackers: "http://t1/announce\nhttp://t2/announce\n\nudp://t3:80".into(),
            web_seeds: "http://seed/files/".into(),
            comment: " hello ".into(),
            piece_size: 32 << 10,
            private: true,
            preserve_order: false,
            format: TorrentFormat::Hybrid,
        };
        let mut calls = Vec::new();
        let c = build(&o, &mut |done, total| {
            calls.push((done, total));
            true
        })
        .unwrap();
        assert_eq!(c.name, "Album");
        // 5,000 and 100,000 bytes in 32 KiB pieces, each file padded to a piece (a hybrid)
        assert_eq!(calls.last(), Some(&(5, 5)));
        assert_eq!(calls.len(), 5);
        let ti = TorrentInfo::from_bytes(&c.bytes).unwrap();
        assert_eq!(ti.info_hash(), c.info_hash);
        assert_eq!(ti.piece_length(), 32 << 10);
        assert!(ti.is_private());
        assert_eq!(ti.comment(), "hello");
        assert!(ti.creator().starts_with("PaTorrent/"));
        // loading shuffles each tier (BEP 12)
        let mut trackers: Vec<(String, u8)> = ti.trackers().iter().map(|a| (a.url.clone(), a.tier)).collect();
        trackers.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
        assert_eq!(
            trackers,
            vec![
                ("http://t1/announce".into(), 0),
                ("http://t2/announce".into(), 0),
                ("udp://t3:80".into(), 1)
            ]
        );
        assert_eq!(ti.web_seeds().iter().map(|w| w.url.as_str()).collect::<Vec<_>>(), ["http://seed/files/"]);
        assert_eq!(file_paths(&c.bytes), ["Album/cover.jpg", "Album/disc 1/01.flac"]);
        // a hybrid by default: both versions, files padded to pieces
        assert!(ti.v1() && ti.v2());
        assert!(ti.files().pad_file_at(1));
    }

    #[test]
    fn each_format() {
        let d = TempDir::new("formats");
        let src = d.0.join("Two");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a"), vec![1u8; 40_000]).unwrap();
        std::fs::write(src.join("b"), vec![2u8; 10_000]).unwrap();
        for (format, v1, v2, pads) in [
            (TorrentFormat::V1, true, false, false),
            (TorrentFormat::Hybrid, true, true, true),
            (TorrentFormat::V2, false, true, true),
        ] {
            let o = CreateOptions { source: src.clone(), format, ..CreateOptions::default() };
            let ti = TorrentInfo::from_bytes(&build(&o, &mut |_, _| true).unwrap().bytes).unwrap();
            assert_eq!((ti.v1(), ti.v2()), (v1, v2), "{format:?}");
            let fs = ti.files();
            assert_eq!((0..fs.num_files()).any(|i| fs.pad_file_at(i)), pads, "{format:?}");
            assert_eq!(TorrentFormat::from_key(format.key()), Some(format));
            assert_eq!(FORMATS[format.index()].1, format);
        }
        assert_eq!(TorrentFormat::from_key("v3"), None);
        assert_eq!(TorrentFormat::default(), TorrentFormat::Hybrid);
    }

    #[test]
    fn single_file_auto_piece_size() {
        let d = TempDir::new("single");
        let f = d.0.join("one.bin");
        std::fs::write(&f, vec![3u8; 70_000]).unwrap();
        let o = CreateOptions { source: f, ..CreateOptions::default() };
        let c = build(&o, &mut |_, _| true).unwrap();
        assert_eq!(c.name, "one.bin");
        let ti = TorrentInfo::from_bytes(&c.bytes).unwrap();
        assert_eq!(ti.files().num_files(), 1);
        assert!(ti.trackers().is_empty());
        assert!(!ti.is_private());
    }

    #[test]
    fn preserve_order_uses_the_listing_order() {
        let d = TempDir::new("order");
        let src = d.0.join("Mixed");
        std::fs::create_dir_all(&src).unwrap();
        for n in ["b.bin", "A.bin", "c.bin"] {
            std::fs::write(src.join(n), vec![9u8; 20_000]).unwrap();
        }
        let listed: Vec<String> = std::fs::read_dir(&src)
            .unwrap()
            .map(|e| format!("Mixed/{}", e.unwrap().file_name().to_string_lossy()))
            .collect();
        let mut o = CreateOptions {
            source: src,
            preserve_order: true,
            format: TorrentFormat::V1,
            ..CreateOptions::default()
        };
        let kept = build(&o, &mut |_, _| true).unwrap();
        assert_eq!(file_paths(&kept.bytes), listed);
        o.preserve_order = false;
        let sorted = build(&o, &mut |_, _| true).unwrap();
        assert_eq!(file_paths(&sorted.bytes), ["Mixed/A.bin", "Mixed/b.bin", "Mixed/c.bin"]);
        // v2 sorts whatever the box says
        o.preserve_order = true;
        o.format = TorrentFormat::Hybrid;
        let hybrid = build(&o, &mut |_, _| true).unwrap();
        assert_eq!(file_paths(&hybrid.bytes), ["Mixed/A.bin", "Mixed/b.bin", "Mixed/c.bin"]);
    }

    #[test]
    fn cancel_and_errors() {
        let d = TempDir::new("cancel");
        let src = d.0.join("big");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("x"), vec![0u8; 200_000]).unwrap();
        let o = CreateOptions { source: src.clone(), piece_size: 16 << 10, ..CreateOptions::default() };
        let mut n = 0;
        let e = build(&o, &mut |_, _| {
            n += 1;
            n < 3
        })
        .unwrap_err();
        assert_eq!(e, "cancelled");

        let all_skipped = CreateOptions { source: src, skip: "*".into(), ..CreateOptions::default() };
        assert!(build(&all_skipped, &mut |_, _| true).unwrap_err().contains("no files"));
        assert!(build(&CreateOptions::default(), &mut |_, _| true).is_err());
        let missing = CreateOptions { source: d.0.join("nope"), ..CreateOptions::default() };
        assert!(build(&missing, &mut |_, _| true).is_err());
    }

    /// The dialog's "Start seeding" path on the real engine: a torrent built from a folder and
    /// added with the folder's parent as save path checks complete and seeds.
    #[test]
    fn created_torrent_seeds_from_the_source_folder() {
        use patorrent_core::{Alert, Session, SessionParams, Settings, TorrentState};
        use std::time::{Duration, Instant};

        let d = TempDir::new("seed");
        let src = d.0.join("Seeded");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.bin"), (0..300_000u32).map(|i| (i * 7) as u8).collect::<Vec<_>>())
            .unwrap();
        std::fs::write(src.join("sub/b.bin"), vec![5u8; 50_000]).unwrap();
        let c = build(&CreateOptions { source: src.clone(), ..CreateOptions::default() }, &mut |_, _| true)
            .unwrap();
        let save = seed_save_path(&src).unwrap();

        let settings = Settings {
            listen_interfaces: "127.0.0.1:0".into(),
            tick_interval: 100,
            enable_dht: false,
            enable_lsd: false,
            enable_upnp: false,
            enable_natpmp: false,
            ..Settings::default()
        };
        let s = Session::start(SessionParams { settings, state_dir: None }).unwrap();
        let h = s.handle();
        h.add_torrent(seed_params(c.bytes, &save));
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut alerts = Vec::new();
        loop {
            assert!(Instant::now() < deadline, "the new torrent did not start seeding");
            h.post_torrent_updates();
            std::thread::sleep(Duration::from_millis(50));
            h.pop_alerts(&mut alerts);
            for a in alerts.drain(..) {
                if let Alert::AddTorrentFailed { error, .. } = &a {
                    panic!("add failed: {error}");
                }
                if let Alert::StateUpdate(v) = a {
                    if let Some(st) = v.iter().find(|st| st.state == TorrentState::Seeding) {
                        // the files plus the pad bytes of the hybrid layout
                        assert_eq!(st.total_done, st.total);
                        assert!(st.total >= 350_000);
                        assert!(st.error.is_empty());
                        return;
                    }
                }
            }
        }
    }
}
