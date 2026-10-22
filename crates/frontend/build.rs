//! Deflates the translated string tables, `lang/<code>.lng` (all but English, whose texts are
//! `src/lang/en.rs`), into `OUT_DIR/<code>.lng.z`, where `lang::LANGUAGES` includes them (D26).
//! A shipped table is the texts alone, one line each (still `.lng`-escaped), in the key order
//! of `lang/en.lng`, which a test keeps equal to `en.rs`; an empty line is a text the table
//! lacks. Without the key names the tables deflate to two thirds. `OUT_DIR/keys.txt` is that
//! key order, for a test to compare with `en.rs`.
//!
//! The encoder is core's, included by path: a build script cannot use a crate it builds beside.
//!
//! It also reads which PatinaTorrent application icon is in use from
//! `assets/patina-icons/app-icon.txt` (one name, such as `01-verdigris`) and passes it to
//! `icons::app_png` as `PATINA_APP_ICON` (`assets/patina-icons/INTEGRATION.md`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{env, fs, thread};

#[path = "../core/src/util/deflate.rs"]
mod deflate;

fn main() {
    println!("cargo:rerun-if-changed=lang");
    println!("cargo:rerun-if-changed=../core/src/util/deflate.rs");
    println!("cargo:rerun-if-changed=../../assets/patina-icons/app-icon.txt");
    let app_icon = fs::read_to_string("../../assets/patina-icons/app-icon.txt")
        .expect("assets/patina-icons/app-icon.txt");
    let app_icon = app_icon.trim();
    assert!(
        Path::new("../../assets/patina-icons/exports/app").join(app_icon).is_dir(),
        "assets/patina-icons/app-icon.txt names {app_icon:?}, not a folder of exports/app"
    );
    println!("cargo:rustc-env=PATINA_APP_ICON={app_icon}");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let english = fs::read_to_string("lang/en.lng").expect("crates/frontend/lang/en.lng");
    let keys: Vec<&str> = entries(&english).map(|(k, _)| k).collect();
    fs::write(out.join("keys.txt"), keys.join("\n")).expect("OUT_DIR/keys.txt");
    let mut tables: Vec<PathBuf> = fs::read_dir("lang")
        .expect("crates/frontend/lang")
        .map(|e| e.expect("crates/frontend/lang").path())
        .filter(|p| p.extension().is_some_and(|x| x == "lng") && p.file_stem().is_some_and(|s| s != "en"))
        .collect();
    tables.sort();
    // One thread per table: build scripts are compiled without optimisation.
    thread::scope(|s| {
        for path in &tables {
            let (out, keys) = (&out, &keys);
            s.spawn(move || compress(path, keys, out));
        }
    });
}

/// A `.lng` file's `(key, text)` lines, comments and blank lines left out.
fn entries(lng: &str) -> impl Iterator<Item = (&str, &str)> {
    lng.lines()
        .map(|l| l.strip_prefix('\u{feff}').unwrap_or(l))
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim(), v)))
}

fn compress(path: &Path, keys: &[&str], out: &Path) {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let texts: HashMap<&str, &str> = entries(&text).collect();
    let shipped: Vec<&str> = keys.iter().map(|k| texts.get(k).copied().unwrap_or_default()).collect();
    let name = path.file_stem().and_then(|s| s.to_str()).expect("a UTF-8 file name");
    let dest = out.join(format!("{name}.lng.z"));
    let packed = deflate::deflate(shipped.join("\n").as_bytes());
    fs::write(&dest, packed).unwrap_or_else(|e| panic!("{}: {e}", dest.display()));
}
