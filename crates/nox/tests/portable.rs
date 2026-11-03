//! Portable mode (D7, D42): a `patorrent-nox` with `patorrent.ini` beside it keeps its state in
//! `nox` beside itself, writes the save paths inside its folder relative to it, and finds them
//! again once the folder has moved. The program is copied into a temporary folder, as a user
//! unpacks it onto a USB stick.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nox-portable-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A copy of `patorrent-nox` in `root`, with the portable marker beside it.
fn portable_copy(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    let exe = root.join(Path::new(env!("CARGO_BIN_EXE_patorrent-nox")).file_name().unwrap());
    std::fs::copy(env!("CARGO_BIN_EXE_patorrent-nox"), &exe).unwrap();
    std::fs::write(root.join("patorrent.ini"), "").unwrap();
    exe
}

fn run(exe: &Path, args: &[&str]) -> String {
    run_in(exe, exe.parent().unwrap(), args)
}

/// `exe` with `args`, started in `cwd`; its output.
fn run_in(exe: &Path, cwd: &Path, args: &[&str]) -> String {
    let out = Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .env_remove("APPIMAGE")
        .env_remove("FLATPAK_ID")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{args:?}: {text}");
    text
}

#[test]
fn a_portable_folder_keeps_its_state_and_moves() {
    let base = temp_dir("move");
    let root = base.join("Stick");
    let exe = portable_copy(&root);
    let downloads = root.join("Downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("data.bin"), vec![7u8; 100_000]).unwrap();
    run(&exe, &["create", &downloads.join("data.bin").to_string_lossy(), "-o", "data.torrent", "--v1"]);

    // the data is there: it is checked, finished, and the process exits with its state saved
    let out = run(
        &exe,
        &[
            "add",
            "data.torrent",
            "--save",
            &downloads.to_string_lossy(),
            "--listen",
            "127.0.0.1:0",
            "--seconds",
            "20",
        ],
    );
    let state = root.join("nox");
    assert!(out.contains(&format!("* state: {}", state.display())), "{out}");
    let saved: Vec<u8> = ["resume.db", "resume.log"]
        .iter()
        .filter_map(|f| std::fs::read(state.join(f)).ok())
        .flatten()
        .collect();
    let saved = String::from_utf8_lossy(&saved);
    assert!(saved.contains("9:save_path11:./Downloads"), "{saved}");
    assert!(!saved.contains(&*downloads.to_string_lossy()), "{saved}");
    assert!(state.join("patorrent.lock").is_file());

    // the folder moved: the torrent's data is found where the folder is now
    let moved = base.join("Moved");
    std::fs::rename(&root, &moved).unwrap();
    let exe = moved.join(exe.file_name().unwrap());
    // started elsewhere, so `./Downloads` is not found by the working directory's chance
    let out = run_in(&exe, &base, &["run", "--listen", "127.0.0.1:0", "--no-update-check", "--seconds", "5"]);
    assert!(out.contains(&format!("* state: {}", moved.join("nox").display())), "{out}");
    assert!(out.contains("* restored 1 torrents"), "{out}");
    assert!(out.contains("100.0 %  [F] Seeding"), "the data was not found in the moved folder: {out}");
    let _ = std::fs::remove_dir_all(&base);
}
