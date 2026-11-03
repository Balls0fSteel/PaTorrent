//! File > Profiles > Create Shortcut (D42): a `.lnk` on the desktop that starts this program in
//! a profile. Through the shell's `ShellLink` object (`IShellLinkW`, saved with `IPersistFile`),
//! whose interfaces windows-sys does not describe: their methods are declared here in the order
//! `ShObjIdl.h` and `ObjIdl.h` give them, the only part of them this module calls.

use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath};
use windows_sys::core::{GUID, HRESULT};

use crate::util::wide;

/// `CLSID_ShellLink`.
const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
const IID_ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const IID_IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);

type Method = unsafe extern "system" fn();

/// `IShellLinkW`'s table: `IUnknown`'s three, then its own eighteen.
#[repr(C)]
struct ShellLinkVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: Method,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_path: Method,
    get_id_list: Method,
    set_id_list: Method,
    get_description: Method,
    set_description: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    get_working_directory: Method,
    set_working_directory: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    get_arguments: Method,
    set_arguments: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    get_hotkey: Method,
    set_hotkey: Method,
    get_show_cmd: Method,
    set_show_cmd: Method,
    get_icon_location: Method,
    set_icon_location: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT,
    set_relative_path: Method,
    resolve: Method,
    set_path: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
}

/// `IPersistFile`'s table: `IUnknown`'s three, `IPersist::GetClassID`, then its own five.
#[repr(C)]
struct PersistFileVtbl {
    query_interface: Method,
    add_ref: Method,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_class_id: Method,
    is_dirty: Method,
    load: Method,
    save: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT,
    save_completed: Method,
    get_cur_file: Method,
}

/// A COM interface pointer, released when dropped.
struct Com(*mut c_void);

impl Com {
    /// The object's method table, as `T`.
    ///
    /// # Safety
    /// The object implements the interface whose table `T` describes.
    unsafe fn vtbl<T>(&self) -> &T {
        // SAFETY: a COM object starts with its table's pointer
        unsafe { &**(self.0 as *const *const T) }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: every interface starts with IUnknown's three methods, Release third
        unsafe { (self.vtbl::<PersistFileVtbl>().release)(self.0) };
    }
}

fn check(hr: HRESULT) -> io::Result<()> {
    if hr >= 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(hr)) }
}

/// The current user's desktop folder.
pub fn desktop() -> Option<PathBuf> {
    // SAFETY: a known folder id and an out pointer freed below
    unsafe {
        let mut p: *mut u16 = null_mut();
        if SHGetKnownFolderPath(&FOLDERID_Desktop, 0, null_mut(), &mut p) != 0 || p.is_null() {
            return None;
        }
        let s = crate::util::from_wide_ptr(p);
        CoTaskMemFree(p as *const c_void);
        Some(PathBuf::from(s))
    }
}

/// Writes the shortcut `link` that starts `target` with `args`, its icon the target's first.
pub fn create(link: &Path, target: &Path, args: &str, description: &str) -> io::Result<()> {
    // SAFETY: COM on this thread for the call's length; every pointer passed lives through it
    unsafe {
        let init = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let result = (|| {
            let mut obj: *mut c_void = null_mut();
            check(CoCreateInstance(
                &CLSID_SHELL_LINK,
                null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_ISHELL_LINK_W,
                &mut obj,
            ))?;
            let shell_link = Com(obj);
            let link_vt = shell_link.vtbl::<ShellLinkVtbl>();
            let target_w = wide(&target.to_string_lossy());
            check((link_vt.set_path)(shell_link.0, target_w.as_ptr()))?;
            check((link_vt.set_arguments)(shell_link.0, wide(args).as_ptr()))?;
            check((link_vt.set_description)(shell_link.0, wide(description).as_ptr()))?;
            check((link_vt.set_icon_location)(shell_link.0, target_w.as_ptr(), 0))?;
            if let Some(dir) = target.parent() {
                check((link_vt.set_working_directory)(shell_link.0, wide(&dir.to_string_lossy()).as_ptr()))?;
            }
            let mut file: *mut c_void = null_mut();
            check((link_vt.query_interface)(shell_link.0, &IID_IPERSIST_FILE, &mut file))?;
            let persist = Com(file);
            let save = persist.vtbl::<PersistFileVtbl>().save;
            check(save(persist.0, wide(&link.to_string_lossy()).as_ptr(), 1))
        })();
        // S_OK and S_FALSE both count; RPC_E_CHANGED_MODE (another model) does not
        if init >= 0 {
            CoUninitialize();
        }
        result
    }
}

/// The shortcut's file name for a profile: `PaTorrent.lnk`, `PaTorrent (work).lnk`.
pub fn file_name(profile: Option<&str>) -> String {
    format!(
        "{}.lnk",
        patorrent_frontend::profile::Profile { name: profile.map(str::to_owned), portable_root: None }
            .label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_starts_its_profile() {
        assert_eq!(file_name(None), "PaTorrent.lnk");
        assert_eq!(file_name(Some("work")), "PaTorrent (work).lnk");
        let dir = std::env::temp_dir().join(format!("pa-lnk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join(file_name(Some("work")));
        let target = std::env::current_exe().unwrap();
        create(&link, &target, "--profile work", "PaTorrent (work)").unwrap();
        let bytes = std::fs::read(&link).unwrap();
        // the Shell Link header (MS-SHLLINK): its size, then CLSID_ShellLink
        assert_eq!(&bytes[..4], &[0x4c, 0, 0, 0]);
        let text = String::from_utf16_lossy(
            &bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>(),
        );
        let odd = String::from_utf16_lossy(
            &bytes[1..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>(),
        );
        assert!(text.contains("--profile work") || odd.contains("--profile work"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
