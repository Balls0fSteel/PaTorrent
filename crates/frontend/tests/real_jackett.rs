//! The search helper against the real Jackett (D51, `docs/SEARCH_HELPER.md` "To verify with a
//! real Jackett"): downloads Jackett's release from GitHub, runs it and searches through it.
//! Ignored: it needs the network, takes minutes, and runs third-party code. Run by hand:
//!
//! `PATORRENT_REAL_JACKETT=<empty folder> cargo test -p patorrent-frontend --test real_jackett -- --ignored --nocapture`
//!
//! The folder stands in for a profile folder; the helper goes into its `search-helper/`.
//! Longer checks on request:
//!
//! - `PATORRENT_REAL_JACKETT_PAUSE=<seconds>`: sleeps after the first start, for a look at what
//!   the helper listens on (`ss -ltnp`, `netstat -an`) and how it runs;
//! - `PATORRENT_REAL_JACKETT_IDLE=1`: waits for the ten-minute idle stop, then adds a result
//!   through the helper's download link, which starts it again;
//! - `PATORRENT_REAL_JACKETT_PROXY=<host>:<port>`: an HTTP proxy the helper is told to use (the
//!   search must still answer; the proxy's log shows the sites' hosts).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use patorrent_frontend::rss::worker::Download;
use patorrent_frontend::search::helper::config::HelperProxy;
use patorrent_frontend::search::helper::{self, api};
use patorrent_frontend::search::{Query, worker};

/// Public sites that need no FlareSolverr and answer a search for "ubuntu" (general sites
/// first: a movie or anime site answers it with nothing), tried in this order.
const SITES: &[&str] = &["thepiratebay", "limetorrents", "torrentgalaxy", "solidtorrents", "nyaasi", "yts"];

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Adds `r` as the GUI would, through the helper's own download link (the magnet the search
/// gave is left out, so the link is followed).
fn add_through_link(
    r: &patorrent_frontend::search::SearchResult,
    source: &patorrent_frontend::search::Source,
) {
    let linked = patorrent_frontend::search::SearchResult { magnet: String::new(), ..r.clone() };
    assert!(!linked.link.is_empty(), "the result has a link: {r:?}");
    let t = Instant::now();
    let dl = worker::fetch_download(&linked, source, None).expect("the download through the helper's link");
    match &dl {
        Download::Magnet(m) => {
            println!("  link {} -> magnet {} in {:?}", r.link, &m[..m.len().min(70)], t.elapsed())
        }
        Download::Torrent(b) => {
            println!("  link {} -> a .torrent of {} bytes in {:?}", r.link, b.len(), t.elapsed())
        }
    }
    let hashes = dl.info_hashes().expect("the download names a torrent");
    println!("  hashes {hashes:?}");
}

#[test]
#[ignore = "downloads and runs the real Jackett; PATORRENT_REAL_JACKETT=<folder>"]
fn the_helper_installs_runs_and_searches() {
    let Some(dir) = std::env::var_os("PATORRENT_REAL_JACKETT") else {
        eprintln!("PATORRENT_REAL_JACKETT is not set: nothing done");
        return;
    };
    let profile = PathBuf::from(dir);
    std::fs::create_dir_all(&profile).unwrap();
    assert!(helper::available(), "the helper is not available on this platform or build");
    let proxy = env("PATORRENT_REAL_JACKETT_PROXY").map(|hp| {
        let (host, port) = hp.rsplit_once(':').expect("host:port");
        HelperProxy {
            kind: 0,
            host: host.to_owned(),
            port: port.parse().unwrap(),
            username: String::new(),
            password: String::new(),
        }
    });
    helper::configure(&profile, proxy.clone());

    // 1. the newest release and its download, checked against the SHA-256
    let t = Instant::now();
    let release = helper::check(None).expect("GitHub's release listing");
    println!(
        "release {} asset {} ({} bytes) in {:?}",
        release.version,
        release.asset.name,
        release.asset.size,
        t.elapsed()
    );
    let t = Instant::now();
    let mut last = 0u64;
    let state = helper::install(&release, None, &mut |done, total| {
        if done - last >= 8 << 20 {
            last = done;
            println!("  {} of {:?}", done, total);
        }
        true
    })
    .expect("download, check and unpack");
    let layout = helper::layout().unwrap();
    println!(
        "installed {} into {} in {:?}; exe {}",
        state.version,
        layout.root.display(),
        t.elapsed(),
        layout.exe().display()
    );
    assert!(layout.exe().is_file(), "the program is where the layout expects it");
    assert!(layout.installed());

    // 2. the first start: health check, our ServerConfig.json kept
    let t = Instant::now();
    let port = helper::ensure_running().expect("the helper starts");
    println!("running on 127.0.0.1:{port} after {:?}", t.elapsed());
    assert!(helper::running());
    let cfg = std::fs::read_to_string(layout.data().join("ServerConfig.json")).expect("ServerConfig.json");
    assert!(cfg.contains(&format!("\"Port\": {port}")) || cfg.contains(&format!("\"Port\":{port}")), "{cfg}");
    assert!(cfg.contains("\"AllowExternal\": false") || cfg.contains("\"AllowExternal\":false"), "{cfg}");
    assert!(cfg.contains(&state.api_key), "our API key is kept: {cfg}");
    println!("ServerConfig.json keeps the port, loopback and our key");
    if let Some(p) = &proxy {
        assert!(cfg.contains("\"ProxyType\": 0") || cfg.contains("\"ProxyType\":0"), "HTTP proxy: {cfg}");
        assert!(
            cfg.contains(&format!("\"ProxyUrl\": \"{}\"", p.host))
                || cfg.contains(&format!("\"ProxyUrl\":\"{}\"", p.host)),
            "{cfg}"
        );
        println!("ServerConfig.json carries the proxy {}:{}", p.host, p.port);
    }
    if let Some(secs) = env("PATORRENT_REAL_JACKETT_PAUSE").and_then(|s| s.parse::<u64>().ok()) {
        println!("pausing {secs} s on port {port} (look at the listener now)");
        std::thread::sleep(Duration::from_secs(secs));
    }

    // 5. the sign-in with the generated password (the hash format)
    let admin = api::Admin::login(port, &state.password).expect("sign-in with the generated password");
    let _ = admin;

    // 6. the site list, FlareSolverr marks
    let sites = helper::list_sites().expect("the public sites");
    let fs = sites.iter().filter(|s| s.needs_flaresolverr).count();
    println!("{} public sites, {} need FlareSolverr", sites.len(), fs);
    assert!(sites.len() > 50, "Jackett knows hundreds of public sites");
    if let Some(x) = sites.iter().find(|s| s.id == "1337x") {
        println!("1337x needs FlareSolverr: {}", x.needs_flaresolverr);
    }
    let pick: Vec<String> = SITES
        .iter()
        .filter(|id| sites.iter().any(|s| &s.id == *id && !s.needs_flaresolverr))
        .take(2)
        .map(|s| s.to_string())
        .collect();
    println!("ticking {pick:?}");
    assert!(!pick.is_empty(), "none of {SITES:?} is available without FlareSolverr");
    helper::apply_sites(&sites, &pick).expect("configuring the ticked sites");
    let after = helper::list_sites().unwrap();
    for id in &pick {
        assert!(after.iter().any(|s| &s.id == id && s.configured), "{id} shows configured");
    }
    println!(
        "configured: {:?}",
        after.iter().filter(|s| s.configured).map(|s| s.id.as_str()).collect::<Vec<_>>()
    );

    // 7. a search through the helper's source, as the GUI's worker does it
    let source = helper::helper_source(&helper::state().unwrap());
    println!("source {} -> {}", source.label(), source.redact(&source.url));
    let query = Query { text: "ubuntu".to_owned(), categories: Vec::new() };
    let t = Instant::now();
    let results = worker::fetch_results(&source, &query, None).expect("a search through the helper");
    println!("{} results in {:?}", results.len(), t.elapsed());
    for r in results.iter().take(5) {
        println!("  {r:?}");
    }
    assert!(!results.is_empty(), "the ticked public sites answer a search for ubuntu");

    // 7b. adding a result through the helper's own link (`/dl/`)
    let first = results.iter().find(|r| !r.link.is_empty()).expect("a result with a link");
    add_through_link(first, &source);

    // 6b. unticking removes the sites
    helper::apply_sites(&after, &[]).expect("unconfiguring");
    let none = helper::list_sites().unwrap();
    assert!(none.iter().all(|s| !s.configured), "nothing configured after unticking");
    helper::apply_sites(&none, &pick).unwrap();

    // 8. the stop: the process is gone
    let pid_before = helper::running();
    helper::stop();
    std::thread::sleep(Duration::from_secs(2));
    assert!(pid_before && !helper::running(), "stopped");
    println!("stopped; was running: {pid_before}");
    let alive = api::healthy(port, Duration::from_secs(2));
    assert!(!alive, "nothing answers on the port after the stop");

    // a second start reuses the port and the configured sites
    let port2 = helper::ensure_running().expect("a second start");
    println!("second start on port {port2}");
    let again = helper::list_sites().unwrap();
    for id in &pick {
        assert!(again.iter().any(|s| &s.id == id && s.configured), "{id} still configured after a restart");
    }

    // 8b. the idle stop, and an add through the link after it (the helper starts again)
    if env("PATORRENT_REAL_JACKETT_IDLE").is_some() {
        let t = Instant::now();
        println!("waiting for the idle stop (ten minutes without a search)...");
        let until = t + Duration::from_secs(13 * 60);
        while helper::running() && Instant::now() < until {
            std::thread::sleep(Duration::from_secs(10));
        }
        assert!(!helper::running(), "the helper stopped for being idle within 13 minutes");
        println!(
            "idle stop after {:?}; nothing on the port: {}",
            t.elapsed(),
            !api::healthy(port2, Duration::from_secs(2))
        );
        let t = Instant::now();
        add_through_link(first, &source);
        println!(
            "the add after the idle stop started it again in {:?}; running: {}",
            t.elapsed(),
            helper::running()
        );
        assert!(helper::running());
    }

    // 10. Update: the same path again keeps the key, the password and the sites
    let t = Instant::now();
    let updated = helper::install(&release, None, &mut |_, _| true).expect("the update");
    println!("updated to {} in {:?}; running after it: {}", updated.version, t.elapsed(), helper::running());
    assert_eq!(updated.api_key, state.api_key, "the key stays");
    assert_eq!(updated.password, state.password, "the password stays");
    let port3 = helper::ensure_running().expect("a start after the update");
    let kept = helper::list_sites().unwrap();
    for id in &pick {
        assert!(kept.iter().any(|s| &s.id == id && s.configured), "{id} still configured after an update");
    }
    let results = worker::fetch_results(&source, &query, None).expect("a search after the update");
    println!("after the update: port {port3}, {} results", results.len());
    assert!(!results.is_empty());

    // 10b. Remove: the folder and the state are gone
    let removed = helper::remove().expect("remove").expect("there was a helper to remove");
    assert_eq!(removed.api_key, state.api_key);
    std::thread::sleep(Duration::from_secs(1));
    assert!(!layout.root.exists(), "{} deleted", layout.root.display());
    assert!(helper::state().is_none() && !helper::installed(), "no state after Remove");
    assert!(!helper::running() && !api::healthy(port3, Duration::from_secs(2)), "nothing runs after Remove");
    println!("removed: folder gone, nothing on port {port3}");
    assert!(helper::ensure_running().is_err(), "a start without a helper is refused");

    // the exit kill, through a fresh install (the files only; no download)
    let state2 = helper::install(&release, None, &mut |_, _| true).expect("a fresh install");
    assert_ne!(state2.api_key, state.api_key, "a fresh install generates a new key");
    let port4 = helper::ensure_running().expect("a start of the fresh install");
    helper::shutdown();
    std::thread::sleep(Duration::from_secs(2));
    assert!(!api::healthy(port4, Duration::from_secs(2)), "killed at exit");
    println!("ok: install, start, sign-in, sites, search, link, stop, restart, update, remove, exit");
}
