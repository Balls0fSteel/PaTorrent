//! Which drive a save path is on, and what kind of drive it is (DISKIO.md part 3, D48): the key
//! one check at a time is kept per, and the type that chooses per-drive defaults. Only asks the
//! OS (no elevation, no new dependencies); the answer may be wrong (USB bridges, RAID, VMs), so it
//! chooses defaults only and is named in the Logger.

// Until the check queue and the per-drive tuning (DISKIO.md parts 2 and 3) use it.
#![cfg_attr(not(test), allow(dead_code))]

use std::fs;
use std::path::{Path, PathBuf};

/// What kind of drive a path is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DriveKind {
    Ssd,
    Hdd,
    Network,
    Removable,
    Unknown,
}

impl DriveKind {
    /// "ssd", "hdd", "network", "removable", "unknown".
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            DriveKind::Ssd => "ssd",
            DriveKind::Hdd => "hdd",
            DriveKind::Network => "network",
            DriveKind::Removable => "removable",
            DriveKind::Unknown => "unknown",
        }
    }

    /// Parses the words `as_str` gives, ignoring case and surrounding spaces.
    pub(crate) fn parse(s: &str) -> Option<DriveKind> {
        let s = s.trim();
        [DriveKind::Ssd, DriveKind::Hdd, DriveKind::Network, DriveKind::Removable, DriveKind::Unknown]
            .into_iter()
            .find(|k| k.as_str().eq_ignore_ascii_case(s))
    }

    /// The word in a description: `SSD`, `HDD`, `network`, `removable`, `unknown`.
    fn label(self) -> &'static str {
        match self {
            DriveKind::Ssd => "SSD",
            DriveKind::Hdd => "HDD",
            k => k.as_str(),
        }
    }
}

/// The drive holding a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DriveId {
    /// What one check at a time is kept per: the physical disk where known (two partitions of
    /// one disk share it), else the volume, else a network share's server. Opaque but stable for
    /// the process's life.
    pub key: String,
    pub kind: DriveKind,
    /// For the Logger, short: `D: (HDD, SATA)`, `/dev/sda (SSD)`, `\\nas (network)`,
    /// `/mnt/x (unknown)`.
    pub describe: String,
}

impl DriveId {
    /// `name (KIND)` or `name (KIND, detail)`.
    fn new(key: String, kind: DriveKind, name: &str, detail: Option<&str>) -> DriveId {
        let describe = match detail {
            Some(d) if !d.is_empty() => format!("{name} ({}, {d})", kind.label()),
            _ => format!("{name} ({})", kind.label()),
        };
        DriveId { key, kind, describe }
    }
}

/// Detects the drive holding `path`. The path may not exist yet: the nearest existing ancestor
/// is asked. Never panics and needs no elevation; kind `Unknown` (keyed by the volume or path)
/// when it cannot tell. Costs a few stat/ioctl calls.
pub(crate) fn detect(path: &Path) -> DriveId {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    imp::detect(&abs)
}

/// `drive_overrides` setting: comma-separated `path=type` pairs (`E:\=hdd, /mnt/nas=network`).
/// The type of the longest override path that is a prefix of `path` (whole components; on
/// Windows `/` and `\` both separate and case is ignored), else None. Malformed pairs are
/// ignored.
pub(crate) fn overridden(overrides: &str, path: &Path) -> Option<DriveKind> {
    let target = components(&path.to_string_lossy());
    let mut best: Option<(usize, DriveKind)> = None;
    for pair in overrides.split(',') {
        let Some((prefix, kind)) = pair.rsplit_once('=') else { continue };
        let Some(kind) = DriveKind::parse(kind) else { continue };
        let prefix = components(prefix.trim());
        if prefix.is_empty() || prefix.len() > target.len() || prefix[..] != target[..prefix.len()] {
            continue;
        }
        if best.is_none_or(|(len, _)| prefix.len() > len) {
            best = Some((prefix.len(), kind));
        }
    }
    best.map(|(_, kind)| kind)
}

/// A path's components for prefix matching: a leading root (`/`, `\`) or UNC marker (`\\`) is a
/// component of its own; empty and `.` components are dropped.
fn components(path: &str) -> Vec<String> {
    #[cfg(windows)]
    let (path, sep) = (strip_verbatim(path).replace('/', "\\").to_lowercase(), '\\');
    #[cfg(not(windows))]
    let (path, sep) = (path.to_string(), '/');
    let mut out = Vec::new();
    let double = format!("{sep}{sep}");
    let rest = if cfg!(windows) && path.starts_with(&double) {
        out.push(double);
        &path[2..]
    } else if path.starts_with(sep) {
        out.push(sep.to_string());
        &path[1..]
    } else {
        &path[..]
    };
    out.extend(rest.split(sep).filter(|c| !c.is_empty() && *c != ".").map(str::to_string));
    out
}

/// `\\?\C:\x` → `C:\x`, `\\?\UNC\server\share` → `\\server\share`; anything else unchanged.
#[cfg(windows)]
fn strip_verbatim(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("\\\\?\\UNC\\").or_else(|| path.strip_prefix("//?/UNC/")) {
        return format!("\\\\{rest}");
    }
    if let Some(rest) = path.strip_prefix("\\\\?\\").or_else(|| path.strip_prefix("//?/")) {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return rest.to_string();
        }
    }
    path.to_string()
}

/// The nearest ancestor of `path` (itself included) that exists, else `path` itself.
fn nearest_existing(path: &Path) -> PathBuf {
    path.ancestors()
        .take_while(|a| !a.as_os_str().is_empty())
        .find(|a| fs::metadata(a).is_ok())
        .unwrap_or(path)
        .to_path_buf()
}

/// The server of a network mount's source, lowercased: `//user@nas/share`, `\\nas\share`,
/// `nas:/export`, `user@nas:/path`, `[fe80::1]:/x`, `https://nas/dav` and `remote:` (rclone).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd", test))]
fn server_of(source: &str) -> Option<String> {
    let s = source.split_once("://").map_or(source, |(_, rest)| rest);
    let s = s.strip_prefix("//").or_else(|| s.strip_prefix("\\\\")).unwrap_or(s);
    let host = if let Some(v6) = s.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        let s = s.split(['/', '\\']).next().unwrap_or("");
        let s = s.split(':').next().unwrap_or("");
        s.rsplit('@').next().unwrap_or("")
    };
    if host.is_empty() { None } else { Some(host.to_lowercase()) }
}

/// glibc's `major()` and `minor()` of a `dev_t`.
#[cfg(any(target_os = "linux", test))]
fn major_minor(dev: u64) -> (u64, u64) {
    let major = ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff);
    let minor = ((dev >> 12) & 0xffff_ff00) | (dev & 0x0000_00ff);
    (major, minor)
}

/// One `/proc/self/mountinfo` line.
#[cfg(any(target_os = "linux", test))]
struct Mount {
    /// `major:minor`.
    dev: String,
    point: String,
    fstype: String,
    source: String,
}

/// Undoes mountinfo's octal escapes (`\040` for a space).
#[cfg(any(target_os = "linux", test))]
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 4 <= b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(any(target_os = "linux", test))]
fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let l: Vec<&str> = left.split(' ').collect();
            let mut r = right.split(' ');
            Some(Mount {
                dev: l.get(2)?.to_string(),
                point: unescape(l.get(4)?),
                fstype: r.next()?.to_string(),
                source: unescape(r.next().unwrap_or("")),
            })
        })
        .collect()
}

/// `disk3s1` → `disk3` (macOS); `ada0p2` → `ada0`, `da1s1a` → `da1`, `nvd0p3` → `nvd0`
/// (FreeBSD): the leading letters and the number after them.
#[cfg(any(target_os = "macos", target_os = "freebsd", test))]
fn bsd_whole_disk(dev: &str) -> &str {
    let letters = dev.bytes().take_while(u8::is_ascii_alphabetic).count();
    let digits = dev[letters..].bytes().take_while(u8::is_ascii_digit).count();
    if letters == 0 || digits == 0 { dev } else { &dev[..letters + digits] }
}

/// The `<rotationrate>` of DISK geom `disk` in FreeBSD's `kern.geom.confxml`: Some(true) when
/// it spins, Some(false) for 0 (non-rotating), None when unknown.
#[cfg(any(target_os = "freebsd", test))]
fn geom_rotating(xml: &str, disk: &str) -> Option<bool> {
    // Geoms hold `<class ref=.../>` but never `</class>`, so this is the whole DISK class.
    let class = &xml[xml.find("<name>DISK</name>")?..];
    let class = &class[..class.find("</class>").unwrap_or(class.len())];
    let name = format!("<name>{disk}</name>");
    // `<geom id=...>` opens a geom; providers and consumers refer back with `<geom ref=.../>`.
    let geom =
        class.split("<geom ").filter(|g| g.starts_with("id=")).find(|g| g.find("<name>") == g.find(&name))?;
    let rate = geom.split("<rotationrate>").nth(1)?.split('<').next()?.trim();
    match rate.parse::<u32>() {
        Ok(0) => Some(false),
        Ok(r) if r > 1 => Some(true),
        _ => None,
    }
}

/// A network drive keyed by its server (or `fallback` when the server is not known).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
fn network(source: Option<&str>, fallback: &str) -> DriveId {
    match source.and_then(server_of) {
        Some(server) => {
            let name = format!("//{server}");
            DriveId::new(name.clone(), DriveKind::Network, &name, None)
        }
        None => DriveId::new(format!("net:{fallback}"), DriveKind::Network, fallback, None),
    }
}

/// Reads a NUL-terminated C string field out of a byte buffer.
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn c_field(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// `statfs` into a zeroed, 8-byte aligned buffer (the structures differ per OS; callers read
/// their fields by offset).
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn statfs_raw(path: &Path) -> Option<Vec<u8>> {
    use std::ffi::{CString, c_char, c_int};
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        #[cfg_attr(all(target_os = "macos", target_arch = "x86_64"), link_name = "statfs$INODE64")]
        fn statfs(path: *const c_char, buf: *mut u64) -> c_int;
    }
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // Larger than either struct (macOS 2168 bytes, FreeBSD 2344).
    let mut buf = vec![0u64; 512];
    // SAFETY: a valid C string and a buffer larger than `struct statfs`.
    if unsafe { statfs(c.as_ptr(), buf.as_mut_ptr()) } != 0 {
        return None;
    }
    Some(buf.iter().flat_map(|w| w.to_ne_bytes()).collect())
}

#[cfg(windows)]
#[allow(non_upper_case_globals)] // matches on windows-sys's `BusType*` constants
mod imp {
    use super::{DriveId, DriveKind, nearest_existing, strip_verbatim};
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    use windows_sys::Win32::Storage::FileSystem::{
        BusType1394, BusTypeAta, BusTypeAtapi, BusTypeFibre, BusTypeFileBackedVirtual, BusTypeMmc,
        BusTypeNvme, BusTypeRAID, BusTypeSCM, BusTypeSas, BusTypeSata, BusTypeScsi, BusTypeSd, BusTypeSpaces,
        BusTypeSsa, BusTypeUfs, BusTypeUsb, BusTypeVirtual, BusTypeiScsi, CreateFileW, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetDriveTypeW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{
        IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_PROPERTY_ID, STORAGE_PROPERTY_QUERY,
        StorageDeviceProperty, StorageDeviceSeekPenaltyProperty,
    };

    // GetDriveTypeW results (WindowsProgramming, a feature core does not otherwise need).
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    /// WNetGetConnectionW: a remembered mapping that is not connected now; the name is still given.
    const ERROR_CONNECTION_UNAVAIL: u32 = 1201;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// `\\server` (lowercased) of a UNC path; None for anything else, `\\.\` and `\\?\` included.
    fn unc_server(path: &str) -> Option<String> {
        let rest = path.strip_prefix("\\\\").or_else(|| path.strip_prefix("//"))?;
        let server = rest.split(['\\', '/']).next().unwrap_or("");
        if server.is_empty() || server == "." || server == "?" {
            return None;
        }
        Some(format!("\\\\{}", server.to_lowercase()))
    }

    fn network(server: String) -> DriveId {
        DriveId::new(server.clone(), DriveKind::Network, &server, None)
    }

    /// `X:` when the path starts with a drive letter.
    fn drive_letter(path: &str) -> Option<String> {
        let b = path.as_bytes();
        (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
            .then(|| format!("{}:", (b[0] as char).to_ascii_uppercase()))
    }

    fn drive_type(root: &str) -> u32 {
        let w = wide(root);
        // SAFETY: a NUL-terminated wide string.
        unsafe { GetDriveTypeW(w.as_ptr()) }
    }

    /// The share a mapped drive letter (`X:`) points to, as `\\server`.
    fn mapped_server(letter: &str) -> Option<String> {
        let w = wide(letter);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: valid in and out buffers; `len` is the out buffer's size in characters.
        let err = unsafe { WNetGetConnectionW(w.as_ptr(), buf.as_mut_ptr(), &mut len) };
        if err != 0 && err != ERROR_CONNECTION_UNAVAIL {
            return None;
        }
        unc_server(&from_wide(&buf))
    }

    /// The mount point of the volume holding an existing path: `C:\`, `C:\mnt\x\`, `\\s\share\`.
    fn volume_path(path: &Path) -> Option<String> {
        let w = wide(&path.to_string_lossy());
        let mut buf = [0u16; 1024];
        // SAFETY: a NUL-terminated input and an out buffer of the given length.
        let ok = unsafe { GetVolumePathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            return None;
        }
        let v = from_wide(&buf);
        (!v.is_empty()).then_some(v)
    }

    /// A handle opened with no access rights: enough for the queries below, without elevation.
    struct Handle(HANDLE);

    impl Handle {
        fn open(name: &str) -> Option<Handle> {
            let w = wide(name);
            // SAFETY: a NUL-terminated name; no security attributes or template.
            let h = unsafe {
                CreateFileW(
                    w.as_ptr(),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            (h != INVALID_HANDLE_VALUE && !h.is_null()).then_some(Handle(h))
        }

        /// DeviceIoControl into an 8-byte aligned buffer; the bytes returned.
        fn ioctl(
            &self,
            code: u32,
            input: Option<&STORAGE_PROPERTY_QUERY>,
            out: &mut [u64],
        ) -> Option<Vec<u8>> {
            let (inp, in_len) = match input {
                Some(q) => (q as *const STORAGE_PROPERTY_QUERY as *const c_void, size_of_val(q) as u32),
                None => (null(), 0),
            };
            let mut returned = 0u32;
            // SAFETY: valid input and output buffers of the given sizes; synchronous call.
            let ok = unsafe {
                DeviceIoControl(
                    self.0,
                    code,
                    inp,
                    in_len,
                    out.as_mut_ptr().cast(),
                    size_of_val(out) as u32,
                    &mut returned,
                    null_mut(),
                )
            };
            if ok == 0 {
                return None;
            }
            let bytes: Vec<u8> = out.iter().flat_map(|w| w.to_ne_bytes()).collect();
            Some(bytes[..(returned as usize).min(bytes.len())].to_vec())
        }

        fn query(&self, property: STORAGE_PROPERTY_ID, out: &mut [u64]) -> Option<Vec<u8>> {
            let q = STORAGE_PROPERTY_QUERY {
                PropertyId: property,
                QueryType: PropertyStandardQuery,
                AdditionalParameters: [0],
            };
            self.ioctl(IOCTL_STORAGE_QUERY_PROPERTY, Some(&q), out)
        }

        /// The physical disk numbers a volume lies on (`VOLUME_DISK_EXTENTS`).
        fn disk_numbers(&self) -> Vec<u32> {
            let mut out = [0u64; 64];
            let Some(b) = self.ioctl(IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, None, &mut out) else {
                return Vec::new();
            };
            let u32_at = |o: usize| b.get(o..o + 4).map(|s| u32::from_ne_bytes([s[0], s[1], s[2], s[3]]));
            let n = u32_at(0).unwrap_or(0) as usize;
            // Each DISK_EXTENT is 24 bytes from offset 8: the disk number, then two i64.
            let mut disks: Vec<u32> = (0..n).filter_map(|i| u32_at(8 + i * 24)).collect();
            disks.sort_unstable();
            disks.dedup();
            disks
        }

        /// Whether the device incurs a seek penalty (`DEVICE_SEEK_PENALTY_DESCRIPTOR`).
        fn seek_penalty(&self) -> Option<bool> {
            let mut out = [0u64; 2];
            let b = self.query(StorageDeviceSeekPenaltyProperty, &mut out)?;
            b.get(8).map(|&v| v != 0)
        }

        /// The bus type and the removable-media flag (`STORAGE_DEVICE_DESCRIPTOR`).
        fn bus(&self) -> Option<(i32, bool)> {
            let mut out = [0u64; 128];
            let b = self.query(StorageDeviceProperty, &mut out)?;
            let bus = b.get(28..32)?;
            Some((i32::from_ne_bytes([bus[0], bus[1], bus[2], bus[3]]), b[10] != 0))
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a handle we opened and own.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn bus_name(bus: i32) -> Option<&'static str> {
        Some(match bus {
            BusTypeScsi => "SCSI",
            BusTypeAtapi => "ATAPI",
            BusTypeAta => "ATA",
            BusType1394 => "1394",
            BusTypeSsa => "SSA",
            BusTypeFibre => "Fibre Channel",
            BusTypeUsb => "USB",
            BusTypeRAID => "RAID",
            BusTypeiScsi => "iSCSI",
            BusTypeSas => "SAS",
            BusTypeSata => "SATA",
            BusTypeSd => "SD",
            BusTypeMmc => "MMC",
            BusTypeVirtual => "Virtual",
            BusTypeFileBackedVirtual => "VHD",
            BusTypeSpaces => "Spaces",
            BusTypeNvme => "NVMe",
            BusTypeSCM => "SCM",
            BusTypeUfs => "UFS",
            _ => return None,
        })
    }

    pub(super) fn detect(abs: &Path) -> DriveId {
        let text = strip_verbatim(&abs.to_string_lossy());
        // Decided from the text first: touching a path on an unreachable server can hang.
        if let Some(server) = unc_server(&text) {
            return network(server);
        }
        if let Some(letter) = drive_letter(&text) {
            match drive_type(&format!("{letter}\\")) {
                DRIVE_REMOTE => return network(mapped_server(&letter).unwrap_or(letter)),
                // A remembered mapping that is not connected now has no root directory.
                DRIVE_NO_ROOT_DIR => {
                    if let Some(server) = mapped_server(&letter) {
                        return network(server);
                    }
                }
                _ => {}
            }
        }
        let existing = nearest_existing(Path::new(&text));
        let root = volume_path(&existing).or_else(|| drive_letter(&text).map(|l| format!("{l}\\")));
        let Some(root) = root else {
            let p = text.to_lowercase();
            return DriveId::new(p.clone(), DriveKind::Unknown, &text, None);
        };
        let dtype = drive_type(&root);
        if dtype == DRIVE_REMOTE {
            let server = unc_server(&root).unwrap_or_else(|| root.trim_end_matches('\\').to_lowercase());
            return network(server);
        }
        let name = match root.trim_end_matches('\\') {
            "" => root.as_str(),
            n => n,
        };

        // The volume device: `\\?\Volume{guid}` (any mount point), else `\\.\X:`.
        let mut guid = [0u16; 128];
        let wroot = wide(&root);
        // SAFETY: a NUL-terminated mount point and an out buffer of the given length.
        let have_guid = unsafe {
            GetVolumeNameForVolumeMountPointW(wroot.as_ptr(), guid.as_mut_ptr(), guid.len() as u32)
        } != 0;
        let device = if have_guid {
            from_wide(&guid).trim_end_matches('\\').to_string()
        } else {
            format!("\\\\.\\{name}")
        };
        let volume = Handle::open(&device);
        let disks = volume.as_ref().map(Handle::disk_numbers).unwrap_or_default();

        // Ask each physical disk; the volume itself (which forwards to its disk) when the
        // extents are not known.
        let mut handles: Vec<Handle> =
            disks.iter().filter_map(|n| Handle::open(&format!("\\\\.\\PhysicalDrive{n}"))).collect();
        if handles.len() != disks.len() || handles.is_empty() {
            handles.clear();
            handles.extend(volume);
        }
        let buses: Vec<(i32, bool)> = handles.iter().filter_map(Handle::bus).collect();
        let penalties: Vec<Option<bool>> = handles.iter().map(Handle::seek_penalty).collect();

        let removable = dtype == DRIVE_REMOVABLE
            || dtype == DRIVE_CDROM
            || buses.iter().any(|&(bus, rm)| rm || matches!(bus, BusTypeUsb | BusTypeSd | BusTypeMmc));
        let kind = if removable {
            DriveKind::Removable
        } else if penalties.contains(&Some(true)) {
            DriveKind::Hdd
        } else if !penalties.is_empty() && penalties.iter().all(|p| *p == Some(false)) {
            DriveKind::Ssd
        } else {
            DriveKind::Unknown
        };
        let key = if disks.is_empty() {
            if have_guid { device.to_lowercase() } else { root.to_lowercase() }
        } else {
            disks.iter().map(|n| format!("PhysicalDrive{n}")).collect::<Vec<_>>().join("+")
        };
        let detail = buses.first().and_then(|&(bus, _)| bus_name(bus));
        DriveId::new(key, kind, name, detail)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn unc_servers() {
            assert_eq!(unc_server(r"\\NAS\share\x").as_deref(), Some(r"\\nas"));
            assert_eq!(unc_server("//nas/share").as_deref(), Some(r"\\nas"));
            assert_eq!(unc_server(r"\\?\Volume{x}\"), None);
            assert_eq!(unc_server(r"\\.\C:"), None);
            assert_eq!(unc_server(r"C:\x"), None);
            assert_eq!(strip_verbatim(r"\\?\UNC\nas\share"), r"\\nas\share");
            assert_eq!(strip_verbatim(r"\\?\D:\x"), r"D:\x");
        }

        #[test]
        fn unc_paths_are_network_without_touching_them() {
            let id = super::super::detect(Path::new(r"\\patorrent-no-such-host.invalid\share\x"));
            assert_eq!(id.kind, DriveKind::Network);
            assert_eq!(id.key, r"\\patorrent-no-such-host.invalid");
            assert_eq!(id.describe, r"\\patorrent-no-such-host.invalid (network)");
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{DriveId, DriveKind, Mount, major_minor, nearest_existing, network, parse_mountinfo};
    use std::ffi::{CString, c_char, c_int};
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    #[cfg(target_arch = "s390x")]
    type FsWord = u32;
    #[cfg(not(target_arch = "s390x"))]
    type FsWord = std::ffi::c_long;

    /// `struct statfs`: only `f_type` (the first field) is read; the rest is room.
    #[repr(C)]
    struct StatFs {
        f_type: FsWord,
        rest: [u64; 63],
    }

    unsafe extern "C" {
        fn statfs(path: *const c_char, buf: *mut StatFs) -> c_int;
    }

    const NFS: u32 = 0x6969;
    const CIFS: u32 = 0xFF53_4D42;
    const SMB2: u32 = 0xFE53_4D42;
    const SMB: u32 = 0x517B;
    const FUSE: u32 = 0x6573_5546;
    /// FUSE file systems that are local (pools and encrypting layers over local directories).
    const LOCAL_FUSE: &[&str] = &["fuse.mergerfs", "fuse.unionfs", "fuse.gocryptfs", "fuse.encfs"];

    fn fs_type(path: &Path) -> Option<u32> {
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut buf = StatFs { f_type: 0, rest: [0; 63] };
        // SAFETY: a valid C string and a buffer larger than `struct statfs`.
        let r = unsafe { statfs(c.as_ptr(), &mut buf) };
        (r == 0).then_some(buf.f_type as u32)
    }

    /// The mount of `path` (device `dev` as `major:minor`): among the mounts of that device, the
    /// one with the longest mount point the path is under.
    fn mount_of(dev: &str, path: &Path) -> Option<Mount> {
        let text = fs::read_to_string("/proc/self/mountinfo").ok()?;
        let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        parse_mountinfo(&text)
            .into_iter()
            .filter(|m| m.dev == dev)
            .max_by_key(|m| if real.starts_with(&m.point) { m.point.len() + 1 } else { 0 })
    }

    fn read_trim(path: &Path) -> Option<String> {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string())
    }

    /// The whole disk of a sysfs block node: a partition's parent, else itself.
    fn whole_disk(node: PathBuf) -> PathBuf {
        if node.join("partition").exists() {
            if let Some(parent) = node.parent() {
                return parent.to_path_buf();
            }
        }
        node
    }

    /// The real disks under a block device: through `slaves/` of `dm-*` and `md*`.
    fn leaves(disk: PathBuf, depth: u32, out: &mut Vec<PathBuf>) {
        let slaves: Vec<PathBuf> = fs::read_dir(disk.join("slaves"))
            .map(|d| d.flatten().filter_map(|e| fs::canonicalize(e.path()).ok()).collect())
            .unwrap_or_default();
        if slaves.is_empty() || depth > 8 {
            if !out.contains(&disk) {
                out.push(disk);
            }
            return;
        }
        for s in slaves {
            leaves(whole_disk(s), depth + 1, out);
        }
    }

    fn name(p: &Path) -> String {
        p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// The drive of block device `major:minor`, from sysfs.
    fn block_drive(major: u64, minor: u64) -> Option<DriveId> {
        let node = fs::canonicalize(format!("/sys/dev/block/{major}:{minor}")).ok()?;
        let top = whole_disk(node);
        let mut disks = Vec::new();
        leaves(top.clone(), 0, &mut disks);
        let rotational: Vec<Option<bool>> =
            disks.iter().map(|d| read_trim(&d.join("queue/rotational")).map(|r| r == "1")).collect();
        let removable = disks.iter().any(|d| {
            read_trim(&d.join("removable")).as_deref() == Some("1") || d.to_string_lossy().contains("/usb")
        });
        let kind = if removable {
            DriveKind::Removable
        } else if rotational.contains(&Some(true)) {
            DriveKind::Hdd
        } else if rotational.iter().all(|r| *r == Some(false)) {
            DriveKind::Ssd
        } else {
            DriveKind::Unknown
        };
        let mut names: Vec<String> = disks.iter().map(|d| name(d)).collect();
        names.sort();
        let key = names.join("+");
        let top_name = name(&top);
        let detail = (names.len() > 1 || names.first() != Some(&top_name)).then(|| key.clone());
        Some(DriveId::new(key, kind, &format!("/dev/{top_name}"), detail.as_deref()))
    }

    pub(super) fn detect(abs: &Path) -> DriveId {
        let existing = nearest_existing(abs);
        let fs_type = fs_type(&existing);
        let dev = fs::metadata(&existing).ok().map(|m| major_minor(m.dev()));
        let dev_text = dev.map(|(a, b)| format!("{a}:{b}")).unwrap_or_default();
        let fallback = existing.to_string_lossy().into_owned();

        if matches!(fs_type, Some(NFS | CIFS | SMB2 | SMB)) {
            let mount = mount_of(&dev_text, &existing);
            let point = mount.as_ref().map_or(fallback.as_str(), |m| m.point.as_str());
            return network(mount.as_ref().map(|m| m.source.as_str()), point);
        }
        // A real block device (fuseblk such as ntfs-3g included).
        if let Some(found) = dev.and_then(|(a, b)| block_drive(a, b)) {
            return found;
        }
        let mount = mount_of(&dev_text, &existing);
        let point = mount.as_ref().map_or(fallback.clone(), |m| m.point.clone());
        if fs_type == Some(FUSE) && mount.as_ref().is_none_or(|m| !LOCAL_FUSE.contains(&m.fstype.as_str())) {
            return network(mount.as_ref().map(|m| m.source.as_str()), &point);
        }
        if let Some(m) = &mount {
            // An anonymous device (btrfs and the like): the mount's source device.
            if m.source.starts_with("/dev/") {
                let rdev = fs::metadata(&m.source).ok().map(|md| major_minor(md.rdev()));
                if let Some(found) = rdev.and_then(|(a, b)| block_drive(a, b)) {
                    return found;
                }
            }
            if m.fstype == "zfs" {
                let pool = m.source.split('/').next().unwrap_or("");
                return DriveId::new(format!("zfs:{pool}"), DriveKind::Unknown, &point, Some("zfs"));
            }
        }
        DriveId::new(format!("mount:{point}"), DriveKind::Unknown, &point, None)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{DriveId, DriveKind, bsd_whole_disk, c_field, nearest_existing, network, statfs_raw};
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::path::Path;

    // `struct statfs` (64-bit inode version) field offsets.
    const FSTYPENAME: usize = 72;
    const MNTONNAME: usize = 88;
    const MNTFROMNAME: usize = 1112;
    const MAXPATHLEN: usize = 1024;

    type CFTypeRef = *const c_void;
    type IoObject = u32;
    const UTF8: u32 = 0x0800_0100;
    const ITERATE_RECURSIVELY_PARENTS: u32 = 1 | 2;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFBooleanGetValue(b: CFTypeRef) -> u8;
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOBSDNameMatching(main_port: u32, options: u32, bsd_name: *const c_char) -> CFTypeRef;
        fn IOServiceGetMatchingService(main_port: u32, matching: CFTypeRef) -> IoObject;
        fn IORegistryEntrySearchCFProperty(
            entry: IoObject,
            plane: *const c_char,
            key: CFTypeRef,
            alloc: CFTypeRef,
            options: u32,
        ) -> CFTypeRef;
        fn IOObjectRelease(object: IoObject) -> c_int;
    }

    /// An owned (+1) CoreFoundation reference.
    struct Cf(CFTypeRef);

    impl Cf {
        fn string(s: &str) -> Option<Cf> {
            let c = CString::new(s).ok()?;
            // SAFETY: a valid C string; the result is owned.
            let r = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) };
            (!r.is_null()).then_some(Cf(r))
        }
    }

    impl Drop for Cf {
        fn drop(&mut self) {
            // SAFETY: an owned, non-null reference.
            unsafe { CFRelease(self.0) };
        }
    }

    /// `value` as a Rust string if it is a CFString (borrowed reference).
    fn cf_string(value: CFTypeRef) -> Option<String> {
        // SAFETY: `value` is a live CF object or null (checked).
        if value.is_null() || unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
            return None;
        }
        let mut buf = [0 as c_char; 256];
        // SAFETY: a CFString and a buffer of the given size.
        let ok = unsafe { CFStringGetCString(value, buf.as_mut_ptr(), buf.len() as isize, UTF8) };
        if ok == 0 {
            return None;
        }
        let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// An IORegistry entry (owned).
    struct Entry(IoObject);

    impl Drop for Entry {
        fn drop(&mut self) {
            // SAFETY: an object we own.
            unsafe { IOObjectRelease(self.0) };
        }
    }

    impl Entry {
        fn for_bsd_name(bsd: &str) -> Option<Entry> {
            let c = CString::new(bsd).ok()?;
            // SAFETY: a valid C string; the matching dictionary is consumed by the second call.
            let service = unsafe {
                let matching = IOBSDNameMatching(0, 0, c.as_ptr());
                if matching.is_null() {
                    return None;
                }
                IOServiceGetMatchingService(0, matching)
            };
            (service != 0).then_some(Entry(service))
        }

        /// A property of this entry or its nearest parent that has it.
        fn search(&self, key: &str) -> Option<Cf> {
            let key = Cf::string(key)?;
            // SAFETY: a live entry, a NUL-terminated plane name and a CFString key; the result
            // is owned.
            let r = unsafe {
                IORegistryEntrySearchCFProperty(
                    self.0,
                    c"IOService".as_ptr(),
                    key.0,
                    std::ptr::null(),
                    ITERATE_RECURSIVELY_PARENTS,
                )
            };
            (!r.is_null()).then_some(Cf(r))
        }

        /// `dict_key` > `key` as a string, where `dict_key` names a dictionary property.
        fn dict_string(&self, dict_key: &str, key: &str) -> Option<String> {
            let dict = self.search(dict_key)?;
            // SAFETY: a live CF object.
            if unsafe { CFGetTypeID(dict.0) != CFDictionaryGetTypeID() } {
                return None;
            }
            let key = Cf::string(key)?;
            // SAFETY: a CFDictionary and a CFString key; the value is borrowed from `dict`.
            cf_string(unsafe { CFDictionaryGetValue(dict.0, key.0) })
        }

        fn flag(&self, key: &str) -> bool {
            let Some(v) = self.search(key) else { return false };
            // SAFETY: a live CF object, read as a CFBoolean only when it is one.
            unsafe { CFGetTypeID(v.0) == CFBooleanGetTypeID() && CFBooleanGetValue(v.0) != 0 }
        }
    }

    pub(super) fn detect(abs: &Path) -> DriveId {
        let existing = nearest_existing(abs);
        let fallback = existing.to_string_lossy().into_owned();
        let Some(buf) = statfs_raw(&existing) else {
            return DriveId::new(format!("path:{fallback}"), DriveKind::Unknown, &fallback, None);
        };
        let fstype = c_field(&buf[FSTYPENAME..FSTYPENAME + 16]);
        let point = c_field(&buf[MNTONNAME..MNTONNAME + MAXPATHLEN]);
        let from = c_field(&buf[MNTFROMNAME..MNTFROMNAME + MAXPATHLEN]);
        if matches!(fstype.as_str(), "smbfs" | "nfs" | "afpfs" | "webdav") {
            return network(Some(&from), &point);
        }
        let Some(bsd) = from.strip_prefix("/dev/") else {
            return DriveId::new(format!("mount:{point}"), DriveKind::Unknown, &point, None);
        };
        let disk = bsd_whole_disk(bsd);
        // The volume's media (a sealed system snapshot may have none), else its whole disk.
        let Some(entry) = Entry::for_bsd_name(bsd).or_else(|| Entry::for_bsd_name(disk)) else {
            return DriveId::new(disk.to_string(), DriveKind::Unknown, &format!("/dev/{disk}"), None);
        };
        let medium = entry.dict_string("Device Characteristics", "Medium Type");
        let bus = entry.dict_string("Protocol Characteristics", "Physical Interconnect");
        let kind = if entry.flag("Removable") || bus.as_deref() == Some("USB") {
            DriveKind::Removable
        } else {
            match medium.as_deref() {
                Some("Solid State") => DriveKind::Ssd,
                Some("Rotational") => DriveKind::Hdd,
                _ => DriveKind::Unknown,
            }
        };
        DriveId::new(disk.to_string(), kind, &format!("/dev/{disk}"), bus.as_deref())
    }
}

#[cfg(target_os = "freebsd")]
mod imp {
    use super::{
        DriveId, DriveKind, bsd_whole_disk, c_field, geom_rotating, nearest_existing, network, statfs_raw,
    };
    use std::ffi::{c_char, c_int, c_void};
    use std::path::Path;

    // `struct statfs` (FreeBSD 12 and later, STATFS_VERSION 0x20140518) field offsets.
    const VERSION: u32 = 0x2014_0518;
    const FSTYPENAME: usize = 280;
    const MNTFROMNAME: usize = 296;
    const MNTONNAME: usize = 1320;
    const MNAMELEN: usize = 1024;

    unsafe extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *const c_void,
            newlen: usize,
        ) -> c_int;
    }

    /// `kern.geom.confxml`, the GEOM tree as XML (readable without privileges).
    fn geom_xml() -> Option<String> {
        for _ in 0..3 {
            let mut len = 0usize;
            // SAFETY: a size query with no buffer.
            if unsafe {
                sysctlbyname(
                    c"kern.geom.confxml".as_ptr(),
                    std::ptr::null_mut(),
                    &mut len,
                    std::ptr::null(),
                    0,
                )
            } != 0
            {
                return None;
            }
            len += len / 4 + 4096;
            let mut buf = vec![0u8; len];
            // SAFETY: a buffer of `len` bytes.
            let r = unsafe {
                sysctlbyname(
                    c"kern.geom.confxml".as_ptr(),
                    buf.as_mut_ptr().cast(),
                    &mut len,
                    std::ptr::null(),
                    0,
                )
            };
            if r == 0 {
                buf.truncate(len);
                return Some(c_field(&buf));
            }
        }
        None
    }

    pub(super) fn detect(abs: &Path) -> DriveId {
        let existing = nearest_existing(abs);
        let fallback = existing.to_string_lossy().into_owned();
        let buf = statfs_raw(&existing).filter(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) == VERSION);
        let Some(buf) = buf else {
            return DriveId::new(format!("path:{fallback}"), DriveKind::Unknown, &fallback, None);
        };
        let fstype = c_field(&buf[FSTYPENAME..FSTYPENAME + 16]);
        let from = c_field(&buf[MNTFROMNAME..MNTFROMNAME + MNAMELEN]);
        let point = c_field(&buf[MNTONNAME..MNTONNAME + MNAMELEN]);
        // `fusefs.sshfs` when the FUSE daemon names a subtype.
        if matches!(fstype.as_str(), "nfs" | "smbfs") || fstype.starts_with("fusefs") {
            return network(Some(&from), &point);
        }
        if fstype == "zfs" {
            let pool = from.split('/').next().unwrap_or("");
            return DriveId::new(format!("zfs:{pool}"), DriveKind::Unknown, &point, Some("zfs"));
        }
        let Some(dev) = from.strip_prefix("/dev/") else {
            return DriveId::new(format!("mount:{point}"), DriveKind::Unknown, &point, None);
        };
        let disk = bsd_whole_disk(dev);
        let kind = match geom_xml().and_then(|x| geom_rotating(&x, disk)) {
            Some(true) => DriveKind::Hdd,
            Some(false) => DriveKind::Ssd,
            None => DriveKind::Unknown,
        };
        DriveId::new(disk.to_string(), kind, &format!("/dev/{disk}"), None)
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))))]
mod imp {
    use super::{DriveId, DriveKind, nearest_existing};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    pub(super) fn detect(abs: &Path) -> DriveId {
        let existing = nearest_existing(abs);
        let name = existing.to_string_lossy().into_owned();
        let key = match std::fs::metadata(&existing) {
            Ok(m) => format!("dev:{}", m.dev()),
            Err(_) => format!("path:{name}"),
        };
        DriveId::new(key, DriveKind::Unknown, &name, None)
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::{DriveId, DriveKind};
    use std::path::Path;

    pub(super) fn detect(abs: &Path) -> DriveId {
        let name = abs.to_string_lossy().into_owned();
        DriveId::new(format!("path:{name}"), DriveKind::Unknown, &name, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip() {
        for k in
            [DriveKind::Ssd, DriveKind::Hdd, DriveKind::Network, DriveKind::Removable, DriveKind::Unknown]
        {
            assert_eq!(DriveKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(DriveKind::parse(" HDD "), Some(DriveKind::Hdd));
        assert_eq!(DriveKind::parse("Network"), Some(DriveKind::Network));
        assert_eq!(DriveKind::parse("hard disk"), None);
        assert_eq!(DriveKind::parse(""), None);
    }

    #[test]
    fn overrides_pick_the_longest_prefix() {
        let o = "/mnt=ssd, /mnt/nas=network, /mnt/nas/fast=hdd";
        assert_eq!(overridden(o, Path::new("/mnt/nas/movies")), Some(DriveKind::Network));
        assert_eq!(overridden(o, Path::new("/mnt/nas/fast/x")), Some(DriveKind::Hdd));
        assert_eq!(overridden(o, Path::new("/mnt/nas")), Some(DriveKind::Network));
        assert_eq!(overridden(o, Path::new("/mnt/other")), Some(DriveKind::Ssd));
        // Whole components only.
        assert_eq!(overridden(o, Path::new("/mntx/y")), None);
        assert_eq!(overridden("/mnt/nas=network", Path::new("/mnt/nasty")), None);
        assert_eq!(overridden(o, Path::new("/home")), None);
        assert_eq!(overridden("", Path::new("/mnt")), None);
        // Order does not matter.
        assert_eq!(overridden("/a/b=hdd,/a=ssd", Path::new("/a/b/c")), Some(DriveKind::Hdd));
        // A trailing separator is the same path.
        assert_eq!(overridden("/srv/=removable", Path::new("/srv/x")), Some(DriveKind::Removable));
    }

    #[test]
    fn malformed_overrides_are_ignored() {
        let o = "garbage, /a=, =hdd, /a=spinning, ,,/a/b=hdd=ssd, /a=hdd";
        assert_eq!(overridden(o, Path::new("/a/x")), Some(DriveKind::Hdd));
        // `/a/b=hdd=ssd` splits at the last `=`: path `/a/b=hdd`, type ssd.
        assert_eq!(overridden(o, Path::new("/a/b=hdd/x")), Some(DriveKind::Ssd));
        assert_eq!(overridden("=hdd", Path::new("/a")), None);
        assert_eq!(overridden("/a", Path::new("/a")), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_overrides_ignore_case_and_separator_style() {
        let o = r"E:\=hdd, e:/Torrents/Fast=ssd, \\NAS\Share=network";
        assert_eq!(overridden(o, Path::new(r"E:\movies")), Some(DriveKind::Hdd));
        assert_eq!(overridden(o, Path::new(r"e:\torrents\fast\x")), Some(DriveKind::Ssd));
        assert_eq!(overridden(o, Path::new("E:/TORRENTS/FAST")), Some(DriveKind::Ssd));
        assert_eq!(overridden(o, Path::new(r"\\nas\share\a")), Some(DriveKind::Network));
        assert_eq!(overridden(o, Path::new(r"\\?\UNC\nas\share\a")), Some(DriveKind::Network));
        assert_eq!(overridden(o, Path::new(r"\\?\E:\a")), Some(DriveKind::Hdd));
        assert_eq!(overridden(o, Path::new(r"\\nas\other")), None);
        assert_eq!(overridden(o, Path::new(r"D:\x")), None);
        assert_eq!(overridden("E:=removable", Path::new(r"E:\x")), Some(DriveKind::Removable));
        // A UNC root is not a drive-relative root.
        assert_eq!(overridden(r"\nas=hdd", Path::new(r"\\nas\x")), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_overrides_are_case_sensitive() {
        assert_eq!(overridden("/Mnt=hdd", Path::new("/mnt/x")), None);
        assert_eq!(overridden("/mnt=hdd", Path::new("/mnt/x")), Some(DriveKind::Hdd));
    }

    #[test]
    fn servers_of_mount_sources() {
        assert_eq!(server_of("//user@NAS/share").as_deref(), Some("nas"));
        assert_eq!(server_of(r"\\nas\share").as_deref(), Some("nas"));
        assert_eq!(server_of("nas:/export/media").as_deref(), Some("nas"));
        assert_eq!(server_of("x@host.lan:/home/x").as_deref(), Some("host.lan"));
        assert_eq!(server_of("x@host.lan:rel/dir").as_deref(), Some("host.lan"));
        assert_eq!(server_of("[fe80::1]:/export").as_deref(), Some("fe80::1"));
        assert_eq!(server_of("https://user@dav.example:8443/remote").as_deref(), Some("dav.example"));
        assert_eq!(server_of("gdrive:").as_deref(), Some("gdrive"));
        assert_eq!(server_of("/dev/sda1"), None);
        assert_eq!(server_of(""), None);
    }

    #[test]
    fn linux_device_numbers() {
        // glibc's `makedev()`.
        let makedev = |major: u64, minor: u64| {
            ((major & 0xffff_f000) << 32)
                | ((major & 0xfff) << 8)
                | ((minor & 0xffff_ff00) << 12)
                | (minor & 0xff)
        };
        for (major, minor) in [(8, 3), (259, 1), (8, 0x103), (0x1234, 0x12345)] {
            assert_eq!(major_minor(makedev(major, minor)), (major, minor));
        }
    }

    #[test]
    fn linux_mountinfo_lines() {
        let text = "36 35 98:0 /mnt1 /mnt/my\\040disk rw,noatime master:1 - ext3 /dev/root rw\n\
                    40 1 0:52 / /mnt/nas rw shared:5 - cifs //NAS/media rw,vers=3.0\n\
                    41 1 0:53 / /mnt/x rw - fuse.sshfs x@host:/srv rw\n\
                    not a mountinfo line\n";
        let m = parse_mountinfo(text);
        assert_eq!(m.len(), 3);
        assert_eq!((m[0].dev.as_str(), m[0].point.as_str()), ("98:0", "/mnt/my disk"));
        assert_eq!((m[0].fstype.as_str(), m[0].source.as_str()), ("ext3", "/dev/root"));
        assert_eq!((m[1].fstype.as_str(), m[1].source.as_str()), ("cifs", "//NAS/media"));
        assert_eq!(server_of(&m[2].source).as_deref(), Some("host"));
        assert_eq!(unescape(r"a\134b\0"), r"a\b\0");
    }

    #[test]
    fn bsd_disk_names() {
        assert_eq!(bsd_whole_disk("disk3s1"), "disk3");
        assert_eq!(bsd_whole_disk("disk12"), "disk12");
        assert_eq!(bsd_whole_disk("ada0p2"), "ada0");
        assert_eq!(bsd_whole_disk("da1s1a"), "da1");
        assert_eq!(bsd_whole_disk("gpt/rootfs"), "gpt/rootfs");
    }

    #[test]
    fn freebsd_geom_rotation_rates() {
        let xml = r#"<mesh><class id="0x1"><name>PART</name><geom id="0x2"><class ref="0x1"/><name>ada0</name>
            <config><rotationrate>7200</rotationrate></config></geom></class>
            <class id="0x3"><name>DISK</name>
            <geom id="0x4"><class ref="0x3"/><name>ada0</name><rank>1</rank><config><rotationrate>0</rotationrate></config>
            <provider id="0x5"><geom ref="0x4"/><name>ada0</name></provider></geom>
            <geom id="0x6"><class ref="0x3"/><name>ada1</name><config><rotationrate>5400</rotationrate></config></geom>
            <geom id="0x7"><class ref="0x3"/><name>vtbd0</name><config><rotationrate>unknown</rotationrate></config></geom>
            </class></mesh>"#;
        assert_eq!(geom_rotating(xml, "ada0"), Some(false));
        assert_eq!(geom_rotating(xml, "ada1"), Some(true));
        assert_eq!(geom_rotating(xml, "vtbd0"), None);
        assert_eq!(geom_rotating(xml, "da9"), None);
        assert_eq!(geom_rotating("<mesh/>", "ada0"), None);
    }

    #[test]
    fn detect_the_temp_dir_and_a_path_not_made_yet() {
        let tmp = std::env::temp_dir();
        let id = detect(&tmp);
        assert!(!id.key.is_empty(), "{id:?}");
        assert!(!id.describe.is_empty(), "{id:?}");
        let later = detect(&tmp.join("patorrent-no-such-dir").join("sub").join("file.bin"));
        assert_eq!(later, id);
        // A relative path is taken from the current directory.
        assert!(!detect(Path::new("no-such-relative-dir/x")).key.is_empty());
    }

    #[test]
    #[ignore = "prints this machine's drives"]
    fn real_drives() {
        let mut paths = vec![std::env::temp_dir(), PathBuf::from(env!("CARGO_MANIFEST_DIR"))];
        if cfg!(windows) {
            // Every letter: a disconnected mapped drive does not exist but is still a network one.
            paths.extend((b'A'..=b'Z').map(|l| PathBuf::from(format!("{}:\\", l as char))));
        } else {
            paths.extend(["/", "/home", "/tmp", "/mnt", "/media", "/srv", "/boot"].map(PathBuf::from));
        }
        for p in paths {
            let start = std::time::Instant::now();
            let id = detect(&p);
            if !p.exists() && id.kind == DriveKind::Unknown {
                continue;
            }
            println!(
                "{:<60} {:<10} {:<30} {} ({:?})",
                p.display(),
                id.kind.as_str(),
                id.key,
                id.describe,
                start.elapsed()
            );
        }
    }
}
