//! The search helper against the real Jackett (D51, `docs/SEARCH_HELPER.md` "To verify with a
//! real Jackett"): downloads Jackett's release from GitHub, runs it and searches through it.
//! Ignored: it needs the network, takes minutes, and runs third-party code. Run by hand:
//!
//! `PATORRENT_REAL_JACKETT=<empty folder> cargo test -p patorrent-frontend --test real_jackett -- --ignored --nocapture`
//!
//! The folder stands in for a profile folder; the helper goes into its `search-helper/`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use patorrent_frontend::search::helper::{self, api};
use patorrent_frontend::search::{Query, worker};

/// Public sites that need no FlareSolverr and answer a search for "ubuntu" (general sites
/// first: a movie or anime site answers it with nothing), tried in this order.
const SITES: &[&str] = &["thepiratebay", "limetorrents", "torrentgalaxy", "solidtorrents", "nyaasi", "yts"];

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
    helper::configure(&profile, None);

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
    helper::shutdown();
    std::thread::sleep(Duration::from_secs(2));
    assert!(!api::healthy(port2, Duration::from_secs(2)), "killed at exit");
    println!("ok: install, start, sign-in, sites, search, stop, restart, exit");
}
