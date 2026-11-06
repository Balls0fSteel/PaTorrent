//! `.torrent` files and `magnet:` links through LaunchServices, as the GTK bundle sets them
//! (`crates/gtk/src/macos.rs`): LaunchServices names programs by bundle id, so only
//! Rookparcel.app can be registered ([`bundle_id`]), and only from its default profile
//! (LaunchServices opens the bundle, never a profile). The `.torrent` type is the UTI
//! LaunchServices has for the extension (`org.bittorrent.torrent`, which the bundle's
//! `Info.plist` imports). macOS has no way to give a type back to "no default", so turning a box
//! off is refused with a pointer to Finder's Get Info (`MacAssocKept`).
//!
//! The four LaunchServices calls are deprecated since macOS 12 in favour of NSWorkspace's
//! (macOS 12 and later only) but still work from 11 to 15; they are declared by hand below.
//! CFString and NSString are toll-free bridged, so NSStrings go in and come out.

use objc2::rc::Retained;
use objc2_foundation::{NSBundle, NSString};
use objc2_uniform_type_identifiers::UTType;
use rookparcel_frontend::integration::mac::{MAGNET_SCHEME, TORRENT_UTI};
use rookparcel_frontend::{profile, tr};

use crate::ui;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSCopyDefaultRoleHandlerForContentType(content_type: *const NSString, role: u32) -> *mut NSString;
    fn LSSetDefaultRoleHandlerForContentType(
        content_type: *const NSString,
        role: u32,
        handler: *const NSString,
    ) -> i32;
    fn LSCopyDefaultHandlerForURLScheme(scheme: *const NSString) -> *mut NSString;
    fn LSSetDefaultHandlerForURLScheme(scheme: *const NSString, handler: *const NSString) -> i32;
}

/// `kLSRolesAll`: the default for opening, viewing and editing alike.
const ROLES_ALL: u32 = 0xFFFF_FFFF;

/// The bundle id of Rookparcel.app when this process runs from it; `None` for a bare
/// executable, which has no `Info.plist`.
pub fn bundle_id() -> Option<String> {
    NSBundle::mainBundle().bundleIdentifier().map(|s| s.to_string()).filter(|s| !s.is_empty())
}

/// Whether the associations can be changed here: from the bundle, in the default profile.
pub fn settable() -> bool {
    bundle_id().is_some() && profile::current().is_default()
}

/// The UTI LaunchServices has for the `.torrent` extension, else [`TORRENT_UTI`] (a dynamic one
/// means nobody declared the type: ours is the one to set).
pub fn torrent_uti() -> String {
    UTType::typeWithFilenameExtension(&NSString::from_str("torrent"))
        .filter(|t| !t.isDynamic())
        .map(|t| t.identifier().to_string())
        .unwrap_or_else(|| TORRENT_UTI.to_owned())
}

/// An owned result of a Copy function.
///
/// # Safety
/// `p` is null or a CFString the caller owns (the Create / Copy rule).
unsafe fn owned(p: *mut NSString) -> Option<String> {
    // SAFETY: the caller's; CFString and NSString are toll-free bridged, and `from_raw` takes
    // over the +1 reference.
    unsafe { Retained::from_raw(p) }.map(|s| s.to_string())
}

/// The bundle id of the default program for the content type `uti`.
fn default_for_type(uti: &str) -> Option<String> {
    let t = NSString::from_str(uti);
    // SAFETY: a valid string; the result follows the Copy rule.
    unsafe { owned(LSCopyDefaultRoleHandlerForContentType(Retained::as_ptr(&t), ROLES_ALL)) }
}

/// The bundle id of the default program for `scheme:` URLs.
fn default_for_scheme(scheme: &str) -> Option<String> {
    let s = NSString::from_str(scheme);
    // SAFETY: a valid string; the result follows the Copy rule.
    unsafe { owned(LSCopyDefaultHandlerForURLScheme(Retained::as_ptr(&s))) }
}

/// LaunchServices' answer: `-10814` (`kLSApplicationNotFoundErr`) means the bundle is not
/// registered with it.
fn status(call: &str, code: i32) -> Result<(), String> {
    if code == 0 { Ok(()) } else { Err(format!("{call}: {code}")) }
}

/// LaunchServices may hand bundle ids back lowercased.
fn is(handler: Option<String>, id: &str) -> bool {
    handler.is_some_and(|h| h.eq_ignore_ascii_case(id))
}

/// Whether Rookparcel.app opens `.torrent` files and `magnet:` links now (false outside the
/// bundle).
pub fn read() -> (bool, bool) {
    let Some(id) = bundle_id() else { return (false, false) };
    (is(default_for_type(&torrent_uti()), &id), is(default_for_scheme(MAGNET_SCHEME), &id))
}

/// Makes Rookparcel.app the program for `.torrent` files and `magnet:` links, each when asked
/// (`Err`: LaunchServices' error, for `ErrAssociate` or `ErrDesktopIntegration`).
pub fn set(torrent: bool, magnet: bool) -> Result<(), String> {
    if !torrent && !magnet {
        return Ok(());
    }
    // the boxes are greyed outside the bundle and the startup check does not ask there: this
    // names what is missing for a log
    let Some(id) = bundle_id() else { return Err("CFBundleIdentifier".into()) };
    let id = NSString::from_str(&id);
    if torrent {
        let t = NSString::from_str(&torrent_uti());
        // SAFETY: valid strings.
        let code = unsafe {
            LSSetDefaultRoleHandlerForContentType(Retained::as_ptr(&t), ROLES_ALL, Retained::as_ptr(&id))
        };
        status("LSSetDefaultRoleHandlerForContentType", code)?;
    }
    if magnet {
        let s = NSString::from_str(MAGNET_SCHEME);
        // SAFETY: valid strings.
        let code = unsafe { LSSetDefaultHandlerForURLScheme(Retained::as_ptr(&s), Retained::as_ptr(&id)) };
        status("LSSetDefaultHandlerForURLScheme", code)?;
    }
    Ok(())
}

/// Preferences > General "Check association on startup": asks to make Rookparcel the program
/// for `.torrent` files and `magnet:` links when it is not (GTK `check_association`).
pub fn check(mtm: objc2::MainThreadMarker) {
    let (torrent, magnet) = read();
    if torrent && magnet {
        return;
    }
    let window = crate::app::ui().map(|u| u.window.clone());
    ui::sheet(
        window.as_deref(),
        ui::Style::Info,
        tr!(AskAssociate),
        "",
        &[tr!(BtnOk), tr!(BtnCancel)],
        move |b| {
            if b != 0 {
                return;
            }
            if let Err(e) = set(!torrent, !magnet) {
                let Some(mtm) = objc2::MainThreadMarker::new() else { return };
                ui::alert(ui::Style::Error, &tr!(ErrAssociate, error = e), "", &[], mtm);
            }
        },
        mtm,
    );
}
