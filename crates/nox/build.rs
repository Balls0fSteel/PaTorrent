//! Embeds the version resource in `patorrent-nox.exe` with the MSVC linker, as
//! `crates/win32/build.rs` does for `patorrent.exe`, whose resource writer this includes.

use std::env;
use std::fs;
use std::path::PathBuf;

include!("../win32/res/resource.rs");

fn main() {
    println!("cargo:rerun-if-changed=../win32/res/resource.rs");
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" && target_env == "msvc" {
        let mut res = res_start();
        version_res(&mut res, "patorrent-nox.exe", "PaTorrent nox");
        let out =
            PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("patorrent-nox.res");
        fs::write(&out, res).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
        println!("cargo:rustc-link-arg-bins={}", out.display());
    }
}
