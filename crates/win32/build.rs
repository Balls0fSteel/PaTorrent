//! Embeds the application manifest, the application icon and the version resource with the
//! MSVC linker (no resource compiler needed).
//!
//! The icon is the PatinaTorrent family named in `assets/patina-icons/app-icon.txt` (see
//! `assets/patina-icons/INTEGRATION.md`): its `.ico` from `exports/app/` becomes an RT_GROUP_ICON
//! (ID 1, [`APP_ICON_ID`], which Explorer shows and `LoadImageW` finds) with one RT_ICON per
//! frame, written with the VERSIONINFO into a compiled resource file (`.res`, `res/resource.rs`).

use std::env;
use std::fs;
use std::path::PathBuf;

include!("res/resource.rs");

/// The icon group's resource ID; `crates/win32/src/icons.rs` loads the same one.
const APP_ICON_ID: u16 = 1;
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
/// MOVEABLE | DISCARDABLE, and PURE for the group, as `rc.exe` writes them.
const FLAGS_ICON: u16 = 0x1010;
const FLAGS_GROUP: u16 = 0x1030;

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let manifest = dir.join("res/patorrent.exe.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed={}", dir.join("res/resource.rs").display());
    let patina = dir.join("../../assets/patina-icons");
    let chosen = patina.join("app-icon.txt");
    println!("cargo:rerun-if-changed={}", chosen.display());
    let family = fs::read_to_string(&chosen).unwrap_or_else(|e| panic!("{}: {e}", chosen.display()));
    let ico = patina.join("exports/app").join(format!("{}.ico", family.trim()));
    println!("cargo:rerun-if-changed={}", ico.display());

    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" && target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
        // trustInfo comes from our manifest; don't let the linker add a second one.
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");

        let bytes = fs::read(&ico).unwrap_or_else(|e| panic!("{}: {e}", ico.display()));
        let mut res = icon_res(&bytes).unwrap_or_else(|e| panic!("{}: {e}", ico.display()));
        version_res(&mut res, "patorrent.exe", "PaTorrent");
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("patorrent.res");
        fs::write(&out, res).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
        println!("cargo:rustc-link-arg-bins={}", out.display());
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// A `.res` with the `.ico` file's frames as RT_ICON 1..=n and their directory as
/// RT_GROUP_ICON [`APP_ICON_ID`].
fn icon_res(ico: &[u8]) -> Result<Vec<u8>, String> {
    if ico.len() < 6 || u16_at(ico, 0) != 0 || u16_at(ico, 2) != 1 {
        return Err("not an icon file".into());
    }
    let count = u16_at(ico, 4) as usize;
    if count == 0 || ico.len() < 6 + 16 * count {
        return Err("no icon directory".into());
    }
    let mut out = res_start();
    let mut group = Vec::with_capacity(6 + 14 * count);
    group.extend(0u16.to_le_bytes());
    group.extend(1u16.to_le_bytes());
    group.extend((count as u16).to_le_bytes());
    for i in 0..count {
        let e = &ico[6 + 16 * i..6 + 16 * (i + 1)];
        let (len, offset) = (u32_at(e, 8) as usize, u32_at(e, 12) as usize);
        let frame = ico.get(offset..offset + len).ok_or_else(|| format!("frame {i} out of range"))?;
        let id = i as u16 + 1;
        resource(&mut out, RT_ICON, id, FLAGS_ICON, frame);
        // GRPICONDIRENTRY: ICONDIRENTRY's first 12 bytes, the frame's ID for the offset
        group.extend(&e[..12]);
        group.extend(id.to_le_bytes());
    }
    resource(&mut out, RT_GROUP_ICON, APP_ICON_ID, FLAGS_GROUP, &group);
    Ok(out)
}
