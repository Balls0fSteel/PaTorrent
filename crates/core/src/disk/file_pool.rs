// Ported from libtorrent-rasterbar 2.0.11: src/file_view_pool.cpp, src/file.cpp;
// fixes from 2.0.12 (3417f6479d)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: pools plain file handles for positional I/O instead of memory-mapped views; LRU by a
// use counter in a hash map (the pool is small, so eviction scans it).

//! A bounded, shared pool of open files.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

/// (storage id, file index).
type Key = (u64, u32);

struct Entry {
    file: Arc<File>,
    writable: bool,
    last_use: u64,
}

struct Inner {
    files: HashMap<Key, Entry>,
    clock: u64,
    limit: usize,
}

/// libtorrent `file_view_pool`: at most `limit` files are open at once, whatever the number of
/// torrents. Handles are shared, so several disk threads can use one file concurrently.
pub(crate) struct FilePool {
    inner: Mutex<Inner>,
}

impl FilePool {
    pub fn new(limit: usize) -> FilePool {
        FilePool { inner: Mutex::new(Inner { files: HashMap::new(), clock: 0, limit: limit.max(1) }) }
    }

    pub fn set_limit(&self, limit: usize) {
        self.lock().limit = limit.max(1);
    }

    /// Returns an open handle for `path`. With `writable`, the file and its parent directories
    /// are created if missing.
    pub fn open(&self, storage: u64, file: u32, path: &Path, writable: bool) -> io::Result<Arc<File>> {
        let key = (storage, file);
        {
            let mut inner = self.lock();
            inner.clock += 1;
            let now = inner.clock;
            if let Some(e) = inner.files.get_mut(&key) {
                if e.writable || !writable {
                    e.last_use = now;
                    return Ok(e.file.clone());
                }
            }
        }
        // Open without holding the lock: opening can be slow (network drives, antivirus).
        let file = Arc::new(open_file(path, writable)?);
        let mut inner = self.lock();
        inner.clock += 1;
        let now = inner.clock;
        if inner.files.len() >= inner.limit && !inner.files.contains_key(&key) {
            if let Some(oldest) = inner.files.iter().min_by_key(|(_, e)| e.last_use).map(|(k, _)| *k) {
                inner.files.remove(&oldest);
            }
        }
        inner.files.insert(key, Entry { file: file.clone(), writable, last_use: now });
        Ok(file)
    }

    /// Closes every file of `storage` (libtorrent `release_files`). Handles still in use by a
    /// running job stay open until that job drops them.
    pub fn release(&self, storage: u64) {
        self.lock().files.retain(|k, _| k.0 != storage);
    }

    #[cfg(test)]
    pub fn open_count(&self) -> usize {
        self.lock().files.len()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub(crate) fn open_file(path: &Path, writable: bool) -> io::Result<File> {
    if !writable {
        return File::open(path);
    }
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    let file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            opts.open(path)?
        }
        Err(e) => return Err(e),
    };
    if file.metadata()?.len() == 0 {
        set_sparse(&file);
    }
    Ok(file)
}

/// Marks a new file sparse, so writing near its end does not make NTFS zero-fill everything
/// before it (libtorrent `file::set_size` / `FSCTL_SET_SPARSE`). Unix files are sparse already.
#[cfg(windows)]
fn set_sparse(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;
    let mut returned = 0u32;
    // SAFETY: the handle is a valid open file for the duration of the call; FSCTL_SET_SPARSE
    // takes no input or output buffer (null + 0 means "set sparse"), and `returned` outlives it.
    unsafe {
        DeviceIoControl(
            file.as_raw_handle() as _,
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
    }
}

#[cfg(not(windows))]
fn set_sparse(_file: &File) {}

/// `path` as a NUL-terminated UTF-16 string for a Win32 file call, in the extended-length form
/// (`\\?\C:\...`, `\\?\UNC\server\share\...`) once it is too long for the plain one, as
/// `std::fs` does for its own calls; without it MoveFileExW and CopyFileExW fail past MAX_PATH
/// unless the process is long-path aware and the OS policy is on.
#[cfg(windows)]
pub(crate) fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    // std's limit: MAX_PATH less room for an 8.3 name, which CreateDirectoryW also needs
    const LIMIT: usize = 248;
    let mut units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if units.len() >= LIMIT && !is_verbatim(&units) {
        // the extended form takes no `/`, `.` or `..`: resolve them first
        units = verbatim(std::path::absolute(path)?.as_os_str().encode_wide().collect());
    }
    units.push(0);
    Ok(units)
}

/// `\\?\...`, `\\.\...` or `\??\...`: a form Win32 passes on unchanged.
#[cfg(windows)]
fn is_verbatim(path: &[u16]) -> bool {
    [r"\\?\", r"\\.\", r"\??\"].iter().any(|pre| path.starts_with(&pre.encode_utf16().collect::<Vec<u16>>()))
}

/// The extended-length form of an absolute, normalized path: `C:\x` → `\\?\C:\x`,
/// `\\server\share\x` → `\\?\UNC\server\share\x`; one already in that form, or a device path,
/// is unchanged.
#[cfg(windows)]
fn verbatim(abs: Vec<u16>) -> Vec<u16> {
    if is_verbatim(&abs) {
        return abs;
    }
    let unc = abs.starts_with(&[u16::from(b'\\'); 2]);
    let (prefix, rest) = if unc { (r"\\?\UNC\", &abs[2..]) } else { (r"\\?\", &abs[..]) };
    prefix.encode_utf16().chain(rest.iter().copied()).collect()
}

/// Renames `old` to `new` unless `new` exists, which fails with `AlreadyExists` (libtorrent
/// 2.0.12 checks first and then renames; this closes the gap where it can). Windows moves
/// without `MOVEFILE_REPLACE_EXISTING`; elsewhere a hard link plus unlinking the old name,
/// falling back to a plain rename where links are unsupported (FAT, some network shares).
#[cfg(windows)]
pub(crate) fn rename_no_replace(old: &Path, new: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let (old, new) = (wide_path(old)?, wide_path(new)?);
    // SAFETY: both buffers are NUL-terminated UTF-16 strings that outlive the call; flags 0
    // means no replace, no copy across volumes, no delayed move.
    if unsafe { MoveFileExW(old.as_ptr(), new.as_ptr(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn rename_no_replace(old: &Path, new: &Path) -> io::Result<()> {
    match std::fs::hard_link(old, new) {
        Ok(()) => std::fs::remove_file(old).inspect_err(|_| {
            let _ = std::fs::remove_file(new);
        }),
        Err(e) if matches!(e.kind(), io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound) => Err(e),
        // No hard links here: check, then rename (the gap libtorrent leaves too).
        Err(_) => match std::fs::symlink_metadata(new) {
            Ok(_) => Err(io::Error::new(io::ErrorKind::AlreadyExists, "file already exists")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => std::fs::rename(old, new),
            Err(e) => Err(e),
        },
    }
}

/// Reads until `buf` is full or the file ends. Returns the bytes read.
pub(crate) fn read_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<usize> {
    let mut total = 0;
    while !buf.is_empty() {
        match pread(file, buf, offset) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                offset += n as u64;
                buf = &mut buf[n..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

pub(crate) fn write_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match pwrite(file, buf, offset) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "failed to write to file")),
            Ok(n) => {
                offset += n as u64;
                buf = &buf[n..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    match file.seek_read(buf, offset) {
        // Reading past the end reports ERROR_HANDLE_EOF on Windows.
        Err(e) if e.raw_os_error() == Some(38) => Ok(0),
        r => r,
    }
}

#[cfg(windows)]
fn pwrite(file: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_write(buf, offset)
}

#[cfg(unix)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

#[cfg(unix)]
fn pwrite(file: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.write_at(buf, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_eviction_and_release() {
        let dir = crate::disk::tests::temp_dir("file_pool");
        let pool = FilePool::new(2);
        let p = |i: u32| dir.join(format!("sub/f{i}"));
        let a = pool.open(1, 0, &p(0), true).unwrap();
        write_at(&a, b"hello", 3).unwrap();
        pool.open(1, 1, &p(1), true).unwrap();
        pool.open(1, 0, &p(0), false).unwrap(); // touch 0: 1 is now the oldest
        pool.open(2, 0, &p(2), true).unwrap();
        assert_eq!(pool.open_count(), 2);
        let mut buf = [0u8; 10];
        let n = read_at(&pool.open(1, 0, &p(0), false).unwrap(), &mut buf, 0).unwrap();
        assert_eq!(&buf[..n], b"\0\0\0hello");
        assert_eq!(read_at(&a, &mut buf, 100).unwrap(), 0, "reading past the end is not an error");
        pool.release(1);
        assert_eq!(pool.open_count(), 1);
        assert!(pool.open(3, 0, &dir.join("missing"), false).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    fn text(units: Vec<u16>) -> String {
        String::from_utf16(&units).unwrap()
    }

    /// BUGHUNT 24: the extended-length form for MoveFileExW / CopyFileExW.
    #[cfg(windows)]
    #[test]
    fn long_paths_take_the_extended_form() {
        let v = |s: &str| text(verbatim(s.encode_utf16().collect()));
        assert_eq!(v(r"C:\a\b"), r"\\?\C:\a\b");
        assert_eq!(v(r"\\nas\share\a"), r"\\?\UNC\nas\share\a");
        assert_eq!(v(r"\\?\C:\a"), r"\\?\C:\a");
        assert_eq!(v(r"\\?\UNC\nas\share\a"), r"\\?\UNC\nas\share\a");
        assert_eq!(v(r"\\.\pipe\x"), r"\\.\pipe\x");

        let w = |s: &str| {
            let mut u = wide_path(Path::new(s)).unwrap();
            assert_eq!(u.pop(), Some(0), "NUL-terminated");
            text(u)
        };
        // short paths pass unchanged, as std::fs passes them
        assert_eq!(w(r"C:\a/b\..\c"), r"C:\a/b\..\c");
        assert_eq!(w(r"rel\x"), r"rel\x");
        let name = "n".repeat(200);
        assert_eq!(w(&format!(r"C:\{name}\x/../{name}")), format!(r"\\?\C:\{name}\{name}"));
        assert_eq!(w(&format!(r"\\nas\share\{name}\{name}")), format!(r"\\?\UNC\nas\share\{name}\{name}"));
        assert_eq!(w(&format!(r"//nas/share/{name}/{name}")), format!(r"\\?\UNC\nas\share\{name}\{name}"));
        let already = format!(r"\\?\C:\{name}\..\{name}");
        assert_eq!(w(&already), already, "an extended path is the caller's as written");
        let rel = w(&format!(r"{name}\{name}"));
        let cwd = std::env::current_dir().unwrap();
        assert!(rel.starts_with(r"\\?\") && rel.ends_with(&format!(r"\{name}\{name}")), "{rel} in {cwd:?}");
    }

    /// A rename past MAX_PATH, which MoveFileExW fails without the extended form.
    #[cfg(windows)]
    #[test]
    fn renames_work_past_max_path() {
        let dir = crate::disk::tests::temp_dir("long-rename");
        let deep = dir.join("d".repeat(120)).join("e".repeat(120));
        std::fs::create_dir_all(&deep).unwrap();
        let (old, new) = (deep.join("o".repeat(100)), deep.join("p".repeat(100)));
        assert!(new.as_os_str().len() > 300);
        std::fs::write(&old, b"x").unwrap();
        rename_no_replace(&old, &new).unwrap();
        assert!(!old.exists());
        assert_eq!(std::fs::read(&new).unwrap(), b"x");
        std::fs::write(&old, b"y").unwrap();
        let e = rename_no_replace(&old, &new).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir_all(dir);
    }
}
