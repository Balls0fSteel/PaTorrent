//! What the GTK GUI needs from macOS itself (PLAN.md D23), beside what GTK's own macOS backend
//! does:
//!
//! - `magnet:` links clicked in a browser. macOS hands them over as a `GURL` Apple event, not on
//!   the command line. GTK 4 (`gtk/gtkapplication-quartz.c`, 4.20) sets an `NSApplication`
//!   delegate whose only open method is `application:openFiles:`, which is what turns Finder's
//!   `odoc` event (a double-clicked `.torrent`) into `g_application_open` when the application
//!   has `G_APPLICATION_HANDLES_OPEN`; it has no `application:openURLs:` and installs no `GURL`
//!   handler, so AppKit drops links. [`install_url_handler`] installs one with the Apple Event
//!   Manager.
//! - "Start Rookparcel on system startup": a per-user LaunchAgent,
//!   `~/Library/LaunchAgents/<id>.plist` with `RunAtLoad` ([`launch_agent`], read back by
//!   [`launch_agent_arguments`]); launchd reads that folder at login.
//! - The `.torrent` and `magnet:` defaults through LaunchServices, which names programs by bundle
//!   id, so only Rookparcel.app can be registered ([`bundle_id`]).
//!
//! The plist text is plain Rust in `rookparcel_frontend::integration::mac`, shared with the
//! native Mac GUI and tested on every system; the bindings below are hand-written against the
//! frameworks (as `rookparcel_frontend::lang`'s Core Foundation one).

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(target_os = "macos")]
pub use rookparcel_frontend::integration::mac::{
    MAGNET_SCHEME, TORRENT_UTI, launch_agent, launch_agent_arguments,
};

#[cfg(target_os = "macos")]
pub use sys::{
    bundle_executable, bundle_id, default_for_scheme, default_for_type, install_url_handler,
    set_default_for_scheme, set_default_for_type, torrent_uti,
};

/// Core Foundation, LaunchServices and the Apple Event Manager (CoreServices).
#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{c_char, c_void};
    use std::path::PathBuf;
    use std::sync::OnceLock;

    type CFIndex = isize;
    type CFTypeRef = *const c_void;
    type OSStatus = i32;
    type OSErr = i16;
    /// `FourCharCode`: `AEEventClass`, `AEEventID`, `AEKeyword`, `DescType`.
    type FourCc = u32;

    /// `AEDesc` (and `AppleEvent`, the same type).
    #[repr(C)]
    pub struct AEDesc {
        descriptor_type: FourCc,
        data_handle: *mut c_void,
    }

    /// `AEEventHandlerProcPtr` (on 64-bit a UPP is the plain function pointer).
    type AEEventHandler =
        extern "C" fn(event: *const AEDesc, reply: *mut AEDesc, refcon: *mut c_void) -> OSErr;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(
            alloc: CFTypeRef,
            bytes: *const u8,
            len: CFIndex,
            encoding: u32,
            external: u8,
        ) -> CFTypeRef;
        fn CFStringGetLength(s: CFTypeRef) -> CFIndex;
        fn CFStringGetMaximumSizeForEncoding(len: CFIndex, encoding: u32) -> CFIndex;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: CFIndex, encoding: u32) -> u8;
        fn CFBundleGetMainBundle() -> CFTypeRef;
        fn CFBundleGetIdentifier(bundle: CFTypeRef) -> CFTypeRef;
        fn CFBundleCopyExecutableURL(bundle: CFTypeRef) -> CFTypeRef;
        fn CFURLGetFileSystemRepresentation(url: CFTypeRef, resolve: u8, buf: *mut u8, size: CFIndex) -> u8;
        fn CFRelease(cf: CFTypeRef);
    }

    // LaunchServices and the AE framework are inside CoreServices (an umbrella that re-exports
    // them). The LaunchServices default-handler calls are deprecated since macOS 12 in favour of
    // NSWorkspace's (Objective-C, with a completion block) but still work.
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSSetDefaultRoleHandlerForContentType(
            content_type: CFTypeRef,
            role: u32,
            bundle_id: CFTypeRef,
        ) -> OSStatus;
        fn LSCopyDefaultRoleHandlerForContentType(content_type: CFTypeRef, role: u32) -> CFTypeRef;
        fn LSSetDefaultHandlerForURLScheme(scheme: CFTypeRef, bundle_id: CFTypeRef) -> OSStatus;
        fn LSCopyDefaultHandlerForURLScheme(scheme: CFTypeRef) -> CFTypeRef;
        fn UTTypeCreatePreferredIdentifierForTag(
            tag_class: CFTypeRef,
            tag: CFTypeRef,
            conforming_to: CFTypeRef,
        ) -> CFTypeRef;
        fn AEInstallEventHandler(
            class: FourCc,
            id: FourCc,
            handler: AEEventHandler,
            refcon: *mut c_void,
            is_sys_handler: u8,
        ) -> OSErr;
        fn AEGetParamPtr(
            event: *const AEDesc,
            keyword: FourCc,
            desired_type: FourCc,
            actual_type: *mut FourCc,
            data: *mut c_void,
            max_size: isize,
            actual_size: *mut isize,
        ) -> OSErr;
    }

    const UTF8: u32 = 0x0800_0100;
    /// `kLSRolesAll`: the default for opening, viewing and editing alike.
    const ROLES_ALL: u32 = 0xFFFF_FFFF;

    const fn fourcc(code: &[u8; 4]) -> FourCc {
        u32::from_be_bytes(*code)
    }
    /// `kInternetEventClass` and `kAEGetURL` are both `'GURL'`.
    const GURL: FourCc = fourcc(b"GURL");
    /// `keyDirectObject`.
    const DIRECT_OBJECT: FourCc = fourcc(b"----");
    /// `typeUTF8Text`.
    const UTF8_TEXT: FourCc = fourcc(b"utf8");

    /// An owned Core Foundation object (the Create / Copy rule), released when dropped. Null
    /// results are `None`: `CFRelease(NULL)` traps (CHANGELOG 2026-10-22_075500_1792648500).
    struct Cf(CFTypeRef);

    impl Cf {
        fn new(p: CFTypeRef) -> Option<Cf> {
            (!p.is_null()).then(|| Cf(p))
        }

        fn string(s: &str) -> Option<Cf> {
            // SAFETY: the bytes and their length come from one &str.
            Cf::new(unsafe {
                CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as CFIndex, UTF8, 0)
            })
        }
    }

    impl Drop for Cf {
        fn drop(&mut self) {
            // SAFETY: owned and not null (`Cf::new`).
            unsafe { CFRelease(self.0) }
        }
    }

    /// A CFString as UTF-8.
    ///
    /// # Safety
    /// `s` is a valid CFString (or null, which gives `None`).
    unsafe fn to_string(s: CFTypeRef) -> Option<String> {
        if s.is_null() {
            return None;
        }
        // SAFETY: a valid CFString (the caller); the buffer's size is passed with it.
        unsafe {
            let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s), UTF8) + 1;
            let mut buf = vec![0u8; usize::try_from(size).ok()?];
            if CFStringGetCString(s, buf.as_mut_ptr().cast(), size, UTF8) == 0 {
                return None;
            }
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            buf.truncate(len);
            String::from_utf8(buf).ok()
        }
    }

    /// The bundle id of Rookparcel.app when this process runs from it (the launcher execs
    /// `Contents/MacOS/rookparcel-gtk`, so Core Foundation finds the bundle around it); `None`
    /// for a bare executable, which has no `Info.plist`.
    pub fn bundle_id() -> Option<String> {
        // SAFETY: the main bundle and its identifier follow the Get rule (not released).
        unsafe {
            let bundle = CFBundleGetMainBundle();
            if bundle.is_null() {
                return None;
            }
            to_string(CFBundleGetIdentifier(bundle)).filter(|s| !s.is_empty())
        }
    }

    /// The bundle's `CFBundleExecutable` (Rookparcel.app's launcher, which sets up GTK's paths
    /// before it becomes the GUI), when running from a bundle.
    pub fn bundle_executable() -> Option<PathBuf> {
        bundle_id()?;
        // SAFETY: the main bundle follows the Get rule, the URL the Copy rule (owned by `Cf`);
        // the buffer's size is passed.
        unsafe {
            let url = Cf::new(CFBundleCopyExecutableURL(CFBundleGetMainBundle()))?;
            let mut buf = vec![0u8; 4096];
            if CFURLGetFileSystemRepresentation(url.0, 1, buf.as_mut_ptr(), buf.len() as CFIndex) == 0 {
                return None;
            }
            let len = buf.iter().position(|&b| b == 0)?;
            buf.truncate(len);
            use std::os::unix::ffi::OsStringExt;
            Some(PathBuf::from(std::ffi::OsString::from_vec(buf)))
        }
    }

    /// The UTI LaunchServices has for the `.torrent` extension (`org.bittorrent.torrent` when
    /// Rookparcel.app or another client declares it), else [`super::TORRENT_UTI`].
    pub fn torrent_uti() -> String {
        let found = (|| {
            let class = Cf::string("public.filename-extension")?;
            let ext = Cf::string("torrent")?;
            // SAFETY: valid CFStrings; the result follows the Create rule (owned by `Cf`).
            let uti =
                Cf::new(unsafe { UTTypeCreatePreferredIdentifierForTag(class.0, ext.0, std::ptr::null()) })?;
            // SAFETY: a valid CFString.
            unsafe { to_string(uti.0) }
        })();
        // a dynamic one ("dyn.…") means nobody declared the type: ours is the one to set
        found.filter(|u| !u.starts_with("dyn.")).unwrap_or_else(|| super::TORRENT_UTI.to_owned())
    }

    /// The bundle id of the default program for the content type `uti`.
    pub fn default_for_type(uti: &str) -> Option<String> {
        let t = Cf::string(uti)?;
        // SAFETY: a valid CFString; the result follows the Copy rule (owned by `Cf`).
        let id = Cf::new(unsafe { LSCopyDefaultRoleHandlerForContentType(t.0, ROLES_ALL) })?;
        // SAFETY: a valid CFString.
        unsafe { to_string(id.0) }
    }

    /// The bundle id of the default program for URLs of `scheme`.
    pub fn default_for_scheme(scheme: &str) -> Option<String> {
        let s = Cf::string(scheme)?;
        // SAFETY: a valid CFString; the result follows the Copy rule (owned by `Cf`).
        let id = Cf::new(unsafe { LSCopyDefaultHandlerForURLScheme(s.0) })?;
        // SAFETY: a valid CFString.
        unsafe { to_string(id.0) }
    }

    fn status(what: &str, code: OSStatus) -> Result<(), String> {
        // -10814 kLSApplicationNotFoundErr: the bundle is not registered with LaunchServices
        if code == 0 { Ok(()) } else { Err(format!("LaunchServices could not set {what} (error {code})")) }
    }

    /// Makes the bundle `bundle_id` the default program for the content type `uti`.
    pub fn set_default_for_type(uti: &str, bundle_id: &str) -> Result<(), String> {
        let (Some(t), Some(id)) = (Cf::string(uti), Cf::string(bundle_id)) else {
            return Err("out of memory".into());
        };
        // SAFETY: valid CFStrings.
        status(uti, unsafe { LSSetDefaultRoleHandlerForContentType(t.0, ROLES_ALL, id.0) })
    }

    /// Makes the bundle `bundle_id` the default program for URLs of `scheme`.
    pub fn set_default_for_scheme(scheme: &str, bundle_id: &str) -> Result<(), String> {
        let (Some(s), Some(id)) = (Cf::string(scheme), Cf::string(bundle_id)) else {
            return Err("out of memory".into());
        };
        // SAFETY: valid CFStrings.
        status(&format!("{scheme}:"), unsafe { LSSetDefaultHandlerForURLScheme(s.0, id.0) })
    }

    static ON_URL: OnceLock<fn(String)> = OnceLock::new();

    /// Hands every `GURL` Apple event's URL (a `magnet:` link clicked in a browser, `open
    /// magnet:…`) to `on_url`, on the main thread: AppKit dispatches Apple events from the event
    /// loop GDK runs. Installed after GTK's startup (`[NSApp finishLaunching]`): the event that
    /// started the application waits in the queue until the main loop runs, so it is not lost.
    pub fn install_url_handler(on_url: fn(String)) -> Result<(), String> {
        let _ = ON_URL.set(on_url);
        // SAFETY: a handler with AEEventHandlerProcPtr's signature that lives for the program.
        let err = unsafe { AEInstallEventHandler(GURL, GURL, get_url, std::ptr::null_mut(), 0) };
        if err == 0 { Ok(()) } else { Err(format!("AEInstallEventHandler: error {err}")) }
    }

    /// The `GURL` handler: the URL is the event's direct object, as text.
    extern "C" fn get_url(event: *const AEDesc, _reply: *mut AEDesc, _refcon: *mut c_void) -> OSErr {
        let mut buf = vec![0u8; 2048];
        loop {
            let (mut kind, mut size) = (0, 0isize);
            // SAFETY: the event is the Apple Event Manager's for this call; the buffer's size
            // is passed and `size` tells how much the whole value needs.
            let err = unsafe {
                AEGetParamPtr(
                    event,
                    DIRECT_OBJECT,
                    UTF8_TEXT,
                    &mut kind,
                    buf.as_mut_ptr().cast(),
                    buf.len() as isize,
                    &mut size,
                )
            };
            if err != 0 {
                return err;
            }
            let Ok(size) = usize::try_from(size) else { return -50 }; // paramErr
            if size > buf.len() {
                buf.resize(size, 0);
                continue;
            }
            buf.truncate(size);
            break;
        }
        if let (Ok(url), Some(on_url)) = (String::from_utf8(buf), ON_URL.get()) {
            on_url(url);
        }
        0
    }
}
