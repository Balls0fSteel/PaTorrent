//! The two archive formats the search helper's releases come in: a zip (Windows) and a
//! gzipped tar (Linux, macOS), unpacked into a folder of ours with the inflater the engine
//! already has ([`rookparcel_core::util::puff`], [`rookparcel_core::util::gzip`]).
//!
//! Both are read whole from memory: the archive was checked against the release's SHA-256
//! before it gets here, so these readers guard against mistakes and hostile names, not against
//! a changed file. Every name is checked by [`safe_path`]: an absolute path, a drive, a `..`
//! or a NUL refuses the whole archive, so nothing is ever written outside the target folder.
//! Links (tar's hard and symbolic links) are skipped, never created. Sizes are capped
//! ([`MAX_TOTAL`], [`MAX_ENTRIES`]).

use std::io;
use std::path::{Path, PathBuf};

use rookparcel_core::util::gzip::inflate_gzip;
use rookparcel_core::util::puff::inflate;

/// The most an archive may unpack to, all entries together.
pub const MAX_TOTAL: u64 = 1 << 30;
/// The most entries an archive may have.
pub const MAX_ENTRIES: usize = 100_000;

/// A name inside an archive as a relative path below the target: `None` for the archive's own
/// root (`./`, an empty name), an error for anything that would leave the target (an
/// absolute path, a drive or UNC prefix, a `..` component) or a NUL.
pub fn safe_path(name: &str) -> Result<Option<PathBuf>, String> {
    let bad = || Err(format!("refused: an archive entry named {name:?}"));
    if name.contains('\0') || name.starts_with(['/', '\\']) {
        return bad();
    }
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return bad(),
            p if p.contains(':') => return bad(),
            p => out.push(p),
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

/// What [`unpack_zip`] and [`unpack_tar_gz`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unpacked {
    pub files: usize,
    pub bytes: u64,
    /// Links skipped (tar).
    pub skipped_links: usize,
}

struct Writer<'a> {
    dest: &'a Path,
    done: Unpacked,
    entries: usize,
}

impl Writer<'_> {
    fn count(&mut self) -> Result<(), String> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err("refused: too many entries in the archive".to_owned());
        }
        Ok(())
    }

    fn dir(&mut self, name: &str) -> Result<(), String> {
        self.count()?;
        if let Some(rel) = safe_path(name)? {
            let p = self.dest.join(rel);
            std::fs::create_dir_all(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        Ok(())
    }

    /// Reserves `size` bytes of the total before they are inflated.
    fn reserve(&mut self, size: u64) -> Result<(), String> {
        self.done.bytes = self.done.bytes.saturating_add(size);
        if self.done.bytes > MAX_TOTAL {
            return Err("refused: the archive unpacks to too much".to_owned());
        }
        Ok(())
    }

    fn file(&mut self, name: &str, data: &[u8], executable: bool) -> Result<(), String> {
        self.count()?;
        let Some(rel) = safe_path(name)? else { return Ok(()) };
        let p = self.dest.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        // a name given twice replaces the first (as unzip and tar do); a link is never
        // written through: whatever is there goes first
        let _ = std::fs::remove_file(&p);
        write_new(&p, data, executable).map_err(|e| format!("{}: {e}", p.display()))?;
        self.done.files += 1;
        Ok(())
    }
}

fn write_new(p: &Path, data: &[u8], executable: bool) -> io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(if executable { 0o755 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = executable;
    let mut f = o.open(p)?;
    f.write_all(data)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

const EOCD: u32 = 0x0605_4b50;
const EOCD64: u32 = 0x0606_4b50;
const EOCD64_LOCATOR: u32 = 0x0706_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;

/// Unpacks the zip archive `data` into `dest` (made if missing). Stored and deflated entries;
/// Zip64 sizes and offsets are understood; an encrypted entry or another method refuses the
/// archive. The central directory names the entries, each local header says where its data
/// starts.
pub fn unpack_zip(data: &[u8], dest: &Path) -> Result<Unpacked, String> {
    let corrupt = |what: &str| format!("not a valid zip archive ({what})");
    // the end record: in the last 64 KiB + 22 bytes (its comment is at most 65535 bytes)
    let from = data.len().saturating_sub(22 + 0xffff);
    let eocd = (from..data.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(data, i) == Some(EOCD))
        .ok_or_else(|| corrupt("no end record"))?;
    let mut count = u64::from(u16_at(data, eocd + 10).ok_or_else(|| corrupt("end record"))?);
    let mut cd_size = u64::from(u32_at(data, eocd + 12).ok_or_else(|| corrupt("end record"))?);
    let mut cd_offset = u64::from(u32_at(data, eocd + 16).ok_or_else(|| corrupt("end record"))?);
    if eocd >= 20 && u32_at(data, eocd - 20) == Some(EOCD64_LOCATOR) {
        let at = u64_at(data, eocd - 20 + 8).ok_or_else(|| corrupt("Zip64 locator"))? as usize;
        if u32_at(data, at) != Some(EOCD64) {
            return Err(corrupt("Zip64 end record"));
        }
        count = u64_at(data, at + 32).ok_or_else(|| corrupt("Zip64 end record"))?;
        cd_size = u64_at(data, at + 40).ok_or_else(|| corrupt("Zip64 end record"))?;
        cd_offset = u64_at(data, at + 48).ok_or_else(|| corrupt("Zip64 end record"))?;
    }
    if count as usize > MAX_ENTRIES {
        return Err("refused: too many entries in the archive".to_owned());
    }
    let cd_end = cd_offset.checked_add(cd_size).filter(|&e| e <= data.len() as u64);
    if cd_end.is_none() {
        return Err(corrupt("central directory"));
    }
    let mut w = Writer { dest, done: Unpacked::default(), entries: 0 };
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut at = cd_offset as usize;
    for _ in 0..count {
        if u32_at(data, at) != Some(CENTRAL) {
            return Err(corrupt("central directory entry"));
        }
        let field16 = |o: usize| u16_at(data, at + o).ok_or_else(|| corrupt("central directory entry"));
        let field32 = |o: usize| u32_at(data, at + o).ok_or_else(|| corrupt("central directory entry"));
        let made_by = field16(4)?;
        let flags = field16(8)?;
        let method = field16(10)?;
        let mut csize = u64::from(field32(20)?);
        let mut usize_ = u64::from(field32(24)?);
        let (name_len, extra_len, comment_len) =
            (field16(28)? as usize, field16(30)? as usize, field16(32)? as usize);
        let external = field32(38)?;
        let mut local = u64::from(field32(42)?);
        let name_bytes = data.get(at + 46..at + 46 + name_len).ok_or_else(|| corrupt("entry name"))?;
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        let extra = data
            .get(at + 46 + name_len..at + 46 + name_len + extra_len)
            .ok_or_else(|| corrupt("extra field"))?;
        zip64_extra(extra, &mut usize_, &mut csize, &mut local);
        at += 46 + name_len + extra_len + comment_len;

        if name.ends_with('/') || name.ends_with('\\') {
            w.dir(&name)?;
            continue;
        }
        if flags & 1 != 0 {
            return Err(format!("{name}: encrypted entries are not supported"));
        }
        let local = local as usize;
        if u32_at(data, local) != Some(LOCAL) {
            return Err(corrupt("local header"));
        }
        let start = local
            + 30
            + u16_at(data, local + 26).ok_or_else(|| corrupt("local header"))? as usize
            + u16_at(data, local + 28).ok_or_else(|| corrupt("local header"))? as usize;
        let raw = start
            .checked_add(csize as usize)
            .and_then(|end| data.get(start..end))
            .ok_or_else(|| corrupt("entry data"))?;
        w.reserve(usize_)?;
        let content = match method {
            0 if csize == usize_ => raw.to_vec(),
            8 => {
                let (out, _) = inflate(raw, usize_ as usize).map_err(|e| format!("{name}: {e}"))?;
                if out.len() as u64 != usize_ {
                    return Err(format!("{name}: the size does not match the directory's"));
                }
                out
            }
            0 => return Err(corrupt("stored sizes differ")),
            m => return Err(format!("{name}: compression method {m} is not supported")),
        };
        // Unix permission bits ride in the high half of the external attributes
        let unix = made_by >> 8 == 3;
        let executable = unix && (external >> 16) & 0o111 != 0;
        w.file(&name, &content, executable)?;
    }
    Ok(w.done)
}

/// Zip64's extended information extra field (0x0001): the 64-bit values of the fields that
/// are all ones, in the order uncompressed size, compressed size, local header offset.
fn zip64_extra(extra: &[u8], usize_: &mut u64, csize: &mut u64, local: &mut u64) {
    let mut i = 0;
    while i + 4 <= extra.len() {
        let (id, len) = (u16_at(extra, i).unwrap_or(0), u16_at(extra, i + 2).unwrap_or(0) as usize);
        let body = extra.get(i + 4..i + 4 + len).unwrap_or(&[]);
        if id == 1 {
            let mut at = 0;
            for v in [usize_, csize, local] {
                if *v == u64::from(u32::MAX) {
                    if let Some(x) = u64_at(body, at) {
                        *v = x;
                    }
                    at += 8;
                }
            }
            return;
        }
        i += 4 + len;
    }
}

/// Unpacks the gzipped tar archive `data` into `dest` (made if missing): ustar, GNU long
/// names (`L`) and pax `path` and `size` records are understood; regular files and folders are
/// written (an executable bit kept on Unix), links skipped.
pub fn unpack_tar_gz(data: &[u8], dest: &Path) -> Result<Unpacked, String> {
    let tar = inflate_gzip(data, MAX_TOTAL as usize + (64 << 20)).map_err(|e| format!("gzip: {e}"))?;
    unpack_tar(&tar, dest)
}

fn octal(field: &[u8]) -> Option<u64> {
    // GNU base-256 for big values: the first byte's high bit set
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        return Some(field[1..].iter().fold(0u64, |a, &b| (a << 8) | u64::from(b)));
    }
    let s = std::str::from_utf8(field).ok()?;
    let s = s.trim_matches(|c: char| c == '\0' || c == ' ');
    if s.is_empty() { Some(0) } else { u64::from_str_radix(s, 8).ok() }
}

fn cstr(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// A pax extended header's records (`<len> <key>=<value>\n`).
fn pax_records(mut data: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let Some(sp) = data.iter().position(|&b| b == b' ') else { break };
        let Some(len) = std::str::from_utf8(&data[..sp]).ok().and_then(|n| n.parse::<usize>().ok()) else {
            break;
        };
        if len <= sp || len > data.len() {
            break;
        }
        let rec = String::from_utf8_lossy(&data[sp + 1..len]).trim_end_matches('\n').to_owned();
        if let Some((k, v)) = rec.split_once('=') {
            out.push((k.to_owned(), v.to_owned()));
        }
        data = &data[len..];
    }
    out
}

/// Unpacks the (inflated) tar archive `tar` into `dest`.
pub fn unpack_tar(tar: &[u8], dest: &Path) -> Result<Unpacked, String> {
    let mut w = Writer { dest, done: Unpacked::default(), entries: 0 };
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut at = 0usize;
    let mut long_name: Option<String> = None;
    let mut pax: Vec<(String, String)> = Vec::new();
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|&b| b == 0) {
            break; // the end
        }
        let corrupt = || "not a valid tar archive".to_owned();
        let mut size = octal(&h[124..136]).ok_or_else(corrupt)?;
        if let Some((_, v)) = pax.iter().find(|(k, _)| k == "size") {
            size = v.parse().map_err(|_| corrupt())?;
        }
        let kind = h[156];
        let body_start = at + 512;
        let body_end =
            body_start.checked_add(size as usize).filter(|&e| e <= tar.len()).ok_or_else(corrupt)?;
        let body = &tar[body_start..body_end];
        at = body_start + (size as usize).div_ceil(512) * 512;
        match kind {
            b'L' => {
                long_name = Some(cstr(body));
                continue;
            }
            b'x' => {
                pax = pax_records(body);
                continue;
            }
            b'g' => continue,
            _ => {}
        }
        let name = match (long_name.take(), pax.iter().find(|(k, _)| k == "path")) {
            (_, Some((_, p))) => p.clone(),
            (Some(n), None) => n,
            (None, None) => {
                let base = cstr(&h[0..100]);
                let prefix = if &h[257..262] == b"ustar" { cstr(&h[345..500]) } else { String::new() };
                if prefix.is_empty() { base } else { format!("{prefix}/{base}") }
            }
        };
        pax.clear();
        match kind {
            b'0' | 0 | b'7' => {
                let mode = octal(&h[100..108]).unwrap_or(0o644);
                w.reserve(size)?;
                w.file(&name, body, mode & 0o111 != 0)?;
            }
            b'5' => w.dir(&name)?,
            b'1' | b'2' => {
                // checked all the same, so a hostile name refuses the archive
                safe_path(&name)?;
                w.count()?;
                w.done.skipped_links += 1;
            }
            _ => w.count()?,
        }
    }
    Ok(w.done)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rookparcel_core::util::deflate::deflate;

    /// A temporary folder removed when dropped.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> TempDir {
            let p = std::env::temp_dir().join(format!(
                "rookparcel-helper-{tag}-{}-{}",
                std::process::id(),
                rookparcel_core::util::random::random_u64()
            ));
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

    /// A zip of `(name, contents, deflated, unix mode)`, built as a zip tool would.
    pub fn zip(entries: &[(&str, &[u8], bool, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, deflated, mode) in entries {
            let packed = if *deflated { deflate(data) } else { data.to_vec() };
            let offset = out.len() as u32;
            let method: u16 = if *deflated { 8 } else { 0 };
            out.extend_from_slice(&LOCAL.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]); // time, date, crc
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&packed);

            central.extend_from_slice(&CENTRAL.to_le_bytes());
            central.extend_from_slice(&(3u16 << 8 | 20).to_le_bytes()); // made by Unix
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 8]); // extra, comment, disk, internal
            central.extend_from_slice(&(mode << 16).to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn tar_header(name: &str, size: usize, kind: u8, mode: u32) -> [u8; 512] {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(format!("{mode:07o}\0").as_bytes());
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h
    }

    /// A tar of `(name, contents, kind, mode)`.
    pub fn tar(entries: &[(&str, &[u8], u8, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, data, kind, mode) in entries {
            out.extend_from_slice(&tar_header(name, data.len(), *kind, *mode));
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        out.extend_from_slice(&[0; 1024]);
        out
    }

    /// `data` as a gzip member.
    pub fn gzip(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
        out.extend_from_slice(&deflate(data));
        out.extend_from_slice(&[0; 8]); // CRC and size, not checked
        out
    }

    #[test]
    fn names_never_leave_the_folder() {
        assert_eq!(safe_path("Jackett/jackett").unwrap(), Some(PathBuf::from("Jackett").join("jackett")));
        assert_eq!(safe_path("./a//b/./c").unwrap(), Some(PathBuf::from("a").join("b").join("c")));
        assert_eq!(safe_path("./").unwrap(), None);
        for bad in ["/etc/passwd", "\\x", "a/../../b", "..", "C:/x", "a/c:x", "a\0b", "a\\..\\b"] {
            assert!(safe_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn zips_unpack() {
        let big = b"Jackett ".repeat(5000);
        let z = zip(&[
            ("Jackett/", b"", false, 0o755),
            ("Jackett/JackettConsole.exe", &big, true, 0o755),
            ("Jackett/readme.txt", b"stored", false, 0o644),
            ("Jackett/Definitions/1337x.yml", b"type: info_flaresolverr", true, 0o644),
        ]);
        let t = TempDir::new("zip");
        let done = unpack_zip(&z, &t.0).unwrap();
        assert_eq!(done.files, 3);
        assert_eq!(std::fs::read(t.0.join("Jackett/JackettConsole.exe")).unwrap(), big);
        assert_eq!(std::fs::read(t.0.join("Jackett/readme.txt")).unwrap(), b"stored");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(t.0.join("Jackett/JackettConsole.exe")).unwrap().permissions().mode();
            assert_eq!(m & 0o111, 0o111);
        }
        // a hostile name refuses the archive, and nothing lands outside
        let t2 = TempDir::new("zip-bad");
        let dest = t2.0.join("in");
        let e = unpack_zip(&zip(&[("../escaped.txt", b"x", false, 0o644)]), &dest).unwrap_err();
        assert!(e.contains("refused"), "{e}");
        assert!(!t2.0.join("escaped.txt").exists());
        assert!(unpack_zip(b"not a zip at all, just text", &dest).is_err());
        let mut cut = z.clone();
        cut.truncate(z.len() / 2);
        assert!(unpack_zip(&cut, &dest).is_err());
    }

    #[test]
    fn tarballs_unpack() {
        let long = format!("Jackett/{}/deep.txt", "d".repeat(120));
        let mut entries: Vec<(&str, &[u8], u8, u32)> = vec![
            ("Jackett/", b"", b'5', 0o755),
            ("Jackett/jackett", b"#!/bin/sh\necho hi\n", b'0', 0o755),
            ("Jackett/link", b"", b'2', 0o777),
            ("Jackett/Content/index.html", b"<html>", b'0', 0o644),
        ];
        // a GNU long name before its entry
        let long_bytes = format!("{long}\0");
        entries.push(("././@LongLink", long_bytes.as_bytes(), b'L', 0));
        entries.push(("Jackett/short", b"long-named", b'0', 0o644));
        // a pax path record
        let rec = "path=Jackett/pax-named.txt\n";
        let pax = format!("{} {rec}", rec.len() + 3);
        entries.push(("PaxHeaders/x", pax.as_bytes(), b'x', 0));
        entries.push(("Jackett/ignored-name", b"pax", b'0', 0o644));
        let t = TempDir::new("tar");
        let done = unpack_tar_gz(&gzip(&tar(&entries)), &t.0).unwrap();
        assert_eq!((done.files, done.skipped_links), (4, 1));
        assert_eq!(std::fs::read(t.0.join(&long)).unwrap(), b"long-named");
        assert_eq!(std::fs::read(t.0.join("Jackett/pax-named.txt")).unwrap(), b"pax");
        assert!(!t.0.join("Jackett/link").exists(), "links are not made");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(t.0.join("Jackett/jackett")).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o755);
            let m = std::fs::metadata(t.0.join("Jackett/Content/index.html")).unwrap().permissions().mode();
            assert_eq!(m & 0o111, 0);
        }
        for bad in ["/abs.txt", "Jackett/../../x"] {
            let e = unpack_tar_gz(&gzip(&tar(&[(bad, b"x", b'0', 0o644)])), &t.0).unwrap_err();
            assert!(e.contains("refused"), "{e}");
        }
        let e = unpack_tar_gz(&gzip(&tar(&[("Jackett/l", b"", b'2', 0), ("/etc/x", b"", b'2', 0)])), &t.0);
        assert!(e.is_err(), "a link's name is checked too");
        assert!(unpack_tar_gz(b"plain", &t.0).is_err());
    }
}
