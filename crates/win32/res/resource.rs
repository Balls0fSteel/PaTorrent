// A compiled resource file (`.res`) written by hand, so no resource compiler is needed:
// `link.exe` takes it as an input like an object file. The format is small: a header per
// resource, then its data, each padded to 4 bytes ("Resource File Formats" in the Win32
// documentation). Shared by the build scripts of patorrent (crates/win32/build.rs) and
// patorrent-nox (crates/nox/build.rs), which `include!` this file.

const RT_VERSION: u16 = 16;
/// MOVEABLE | PURE, as `rc.exe` writes a VERSIONINFO.
const FLAGS_VERSION: u16 = 0x0030;

/// A `.res` starts with an empty entry.
fn res_start() -> Vec<u8> {
    let mut out = Vec::new();
    resource(&mut out, 0, 0, 0, &[]);
    out
}

/// One resource: its 32-byte header (numeric type and name), the data, padding to 4 bytes.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes()); // DataSize
    out.extend(32u32.to_le_bytes()); // HeaderSize
    out.extend([0xff, 0xff]);
    out.extend(kind.to_le_bytes()); // TYPE
    out.extend([0xff, 0xff]);
    out.extend(id.to_le_bytes()); // NAME
    out.extend(0u32.to_le_bytes()); // DataVersion
    out.extend(flags.to_le_bytes()); // MemoryFlags
    out.extend(0u16.to_le_bytes()); // LanguageId: neutral
    out.extend(0u32.to_le_bytes()); // Version
    out.extend(0u32.to_le_bytes()); // Characteristics
    out.extend(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

/// The VERSIONINFO resource (ID 1) that Explorer's Details tab and Task Manager show, as
/// µTorrent's exe had one: the package version from Cargo as four numbers (`1.0.0-rc.1` is
/// 1.0.0.0, flagged as a pre-release, as the Windows manifest numbers it) and as text.
fn version_res(out: &mut Vec<u8>, file_name: &str, description: &str) {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let num = |k: &str| var(k).parse::<u16>().unwrap_or(0) as u32;
    let (major, minor, patch) = (num("CARGO_PKG_VERSION_MAJOR"), num("CARGO_PKG_VERSION_MINOR"), num("CARGO_PKG_VERSION_PATCH"));
    let version = var("CARGO_PKG_VERSION");
    let mut fixed = Vec::with_capacity(52);
    for v in [
        0xfeef_04bd,           // dwSignature
        0x0001_0000,           // dwStrucVersion
        major << 16 | minor,   // dwFileVersionMS
        patch << 16,           // dwFileVersionLS
        major << 16 | minor,   // dwProductVersionMS
        patch << 16,           // dwProductVersionLS
        0x3f,                  // dwFileFlagsMask
        if var("CARGO_PKG_VERSION_PRE").is_empty() { 0 } else { 2 }, // VS_FF_PRERELEASE
        0x0004_0004,           // dwFileOS: VOS_NT_WINDOWS32
        1,                     // dwFileType: VFT_APP
        0,                     // dwFileSubtype
        0,                     // dwFileDateMS
        0,                     // dwFileDateLS
    ] {
        fixed.extend(u32::to_le_bytes(v));
    }
    let text = |key: &str, value: &str| {
        let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
        let bytes: Vec<u8> = value.iter().flat_map(|u| u.to_le_bytes()).collect();
        version_block(key, &bytes, value.len() as u16, true, &[])
    };
    let strings = [
        text("CompanyName", &var("CARGO_PKG_AUTHORS")),
        text("FileDescription", description),
        text("FileVersion", &version),
        text("InternalName", file_name.trim_end_matches(".exe")),
        text("LegalCopyright", &format!("Copyright 2026 {}", var("CARGO_PKG_AUTHORS"))),
        text("OriginalFilename", file_name),
        text("ProductName", "PaTorrent"),
        text("ProductVersion", &version),
    ];
    // U.S. English (0x0409), Unicode (1200 = 0x04b0)
    let table = version_block("040904B0", &[], 0, true, &strings);
    let string_info = version_block("StringFileInfo", &[], 0, true, &[table]);
    let translation = version_block("Translation", &[0x09, 0x04, 0xb0, 0x04], 4, false, &[]);
    let var_info = version_block("VarFileInfo", &[], 0, true, &[translation]);
    let info = version_block("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[string_info, var_info]);
    resource(out, RT_VERSION, 1, FLAGS_VERSION, &info);
}

/// A VERSIONINFO block: `wLength`, `wValueLength` (bytes for binary data, characters for
/// text), `wType` (1 for text), the key (UTF-16 and a NUL, padded to 4 bytes), the value, then
/// the children, each starting on a 4-byte boundary.
fn version_block(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0; 6];
    b.extend(key.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    b.resize(b.len().next_multiple_of(4), 0);
    b.extend(value);
    for c in children {
        b.resize(b.len().next_multiple_of(4), 0);
        b.extend(c);
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&value_len.to_le_bytes());
    b[4..6].copy_from_slice(&u16::from(text).to_le_bytes());
    b
}
