//! `patorrent-nox create`: makes a v1 `.torrent` from a file or directory.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use patorrent_core::VERSION;
use patorrent_core::metainfo::create_torrent::parent_dir;
use patorrent_core::metainfo::{
    CreateFlags, CreateTorrent, FileStorage, TorrentInfo, add_files, make_magnet_uri_from_info,
    set_piece_hashes,
};
use patorrent_frontend::fmt;

pub struct CreateOpts {
    path: PathBuf,
    output: Option<PathBuf>,
    /// One entry per tier.
    trackers: Vec<Vec<String>>,
    web_seeds: Vec<String>,
    nodes: Vec<(String, u16)>,
    private: bool,
    piece_size: u32,
    comment: String,
    source: String,
    creator: Option<String>,
    no_date: bool,
    flags: CreateFlags,
    skip_hidden: bool,
    magnet: bool,
}

pub fn parse(args: &[String]) -> Result<CreateOpts, String> {
    let mut o = CreateOpts {
        path: PathBuf::new(),
        output: None,
        trackers: Vec::new(),
        web_seeds: Vec::new(),
        nodes: Vec::new(),
        private: false,
        piece_size: 0,
        comment: String::new(),
        source: String::new(),
        creator: None,
        no_date: false,
        flags: CreateFlags::V1_ONLY,
        skip_hidden: false,
        magnet: false,
    };
    let mut path = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-o" | "--output" => o.output = Some(value(a)?.into()),
            "-t" | "--tracker" => {
                let tier: Vec<String> = value(a)?
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                if !tier.is_empty() {
                    o.trackers.push(tier);
                }
            }
            "-w" | "--web-seed" => o.web_seeds.push(value(a)?),
            "--node" => {
                let v = value(a)?;
                let (host, port) =
                    v.rsplit_once(':').ok_or_else(|| format!("--node {v}: expected HOST:PORT"))?;
                let port = port.parse().map_err(|_| format!("--node {v}: bad port"))?;
                o.nodes.push((host.trim_matches(['[', ']']).to_owned(), port));
            }
            "--private" => o.private = true,
            "--piece-size" => {
                let kib: u32 = value(a)?.parse().map_err(|_| "--piece-size: not a number".to_owned())?;
                o.piece_size = kib.checked_mul(1024).ok_or("--piece-size: too large")?;
            }
            "--comment" => o.comment = value(a)?,
            "--source" => o.source = value(a)?,
            "--creator" => o.creator = Some(value(a)?),
            "--no-date" => o.no_date = true,
            "--pad" => o.flags |= CreateFlags::CANONICAL_FILES,
            "--hybrid" => o.flags.remove(CreateFlags::V1_ONLY | CreateFlags::V2_ONLY),
            "--v2" => {
                o.flags.remove(CreateFlags::V1_ONLY);
                o.flags.insert(CreateFlags::V2_ONLY);
            }
            "--mtime" => o.flags |= CreateFlags::MODIFICATION_TIME,
            "--symlinks" => o.flags |= CreateFlags::SYMLINKS,
            "--no-attributes" => o.flags |= CreateFlags::NO_ATTRIBUTES,
            "--skip-hidden" => o.skip_hidden = true,
            "--magnet" => o.magnet = true,
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("create: unknown option {s}")),
            s if path.is_none() => path = Some(PathBuf::from(s)),
            s => return Err(format!("create: more than one path ({s})")),
        }
    }
    o.path = path.ok_or("create: no file or directory given")?;
    Ok(o)
}

pub fn create(o: CreateOpts) -> ExitCode {
    match make(&o) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn make(o: &CreateOpts) -> Result<(), String> {
    let started = Instant::now();
    let mut fs = FileStorage::new();
    let root = o.path.clone();
    let skip_hidden = o.skip_hidden;
    let mut pred = |p: &Path| {
        // the path given on the command line is always taken
        !skip_hidden || p == root || !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
    };
    add_files(&mut fs, &o.path, &mut pred, o.flags).map_err(|e| e.to_string())?;
    if fs.num_files() == 0 {
        return Err(format!("{}: no files found", o.path.display()));
    }
    let mut t = CreateTorrent::new(fs, o.piece_size, o.flags).map_err(|e| e.to_string())?;
    for (tier, urls) in o.trackers.iter().enumerate() {
        for url in urls {
            t.add_tracker(url, tier as u32);
        }
    }
    for w in &o.web_seeds {
        t.add_url_seed(w);
    }
    for (host, port) in &o.nodes {
        t.add_node(host.clone(), *port);
    }
    t.set_priv(o.private);
    t.set_comment(&o.comment);
    t.set_source(&o.source);
    t.set_creator(o.creator.as_deref().unwrap_or(&format!("PaTorrent/{VERSION}")));
    if o.no_date {
        t.set_creation_date(0);
    }

    let files = t.files();
    println!(
        "{}: {} file(s), {}, {} pieces of {}",
        files.name(),
        files.num_files(),
        fmt::size(files.total_size()),
        t.num_pieces(),
        fmt::size(t.piece_length() as u64)
    );

    let base = parent_dir(&o.path).map_err(|e| format!("{}: {e}", o.path.display()))?;
    let num_pieces = t.num_pieces();
    let mut last_print = Instant::now();
    let mut stdout = std::io::stdout();
    set_piece_hashes(&mut t, &base, &mut |done| {
        let done = done + 1;
        if done == num_pieces || last_print.elapsed().as_millis() >= 250 {
            last_print = Instant::now();
            let _ = write!(
                stdout,
                "\rhashing: {done}/{num_pieces} ({:.1} %)",
                done as f64 * 100.0 / num_pieces as f64
            );
            let _ = stdout.flush();
        }
        true
    })
    .map_err(|e| format!("\n{e}"))?;
    println!();

    let buf = t.generate_buf().map_err(|e| e.to_string())?;
    // check that what we wrote loads, and report its info-hash
    let ti = TorrentInfo::from_bytes(&buf).map_err(|e| format!("created torrent does not load: {e}"))?;
    let out = o.output.clone().unwrap_or_else(|| PathBuf::from(format!("{}.torrent", ti.name())));
    std::fs::write(&out, &buf).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("info-hash: {}", ti.info_hash().to_hex());
    if o.magnet {
        println!("{}", make_magnet_uri_from_info(&ti));
    }
    println!("wrote {} in {:.1} s", out.display(), started.elapsed().as_secs_f64());
    Ok(())
}
