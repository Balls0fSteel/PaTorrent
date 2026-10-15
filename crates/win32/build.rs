//! Embeds the application manifest with the MSVC linker (no resource compiler needed).

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("res/libretorrent.exe.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" && target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
        // trustInfo comes from our manifest; don't let the linker add a second one.
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");
    }
}
