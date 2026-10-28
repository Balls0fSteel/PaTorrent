//! The Windows icons, at the window's DPI: the toolbar images from the artwork PNGs
//! `patorrent_frontend::icons` embeds (decoded there, for the disabled copies), the application icon from the executable's icon resource
//! (`build.rs` embeds the PatinaTorrent `.ico`), and the PatinaTorrent state icons from the
//! PNGs the frontend embeds (Windows decodes PNG icon images itself), as are the navigation
//! symbols of the category list, the detail tabs and the Preferences pages.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, ILC_COLOR32, ILC_MIRROR, ImageList_Add, ImageList_Create, ImageList_ReplaceIcon,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, HICON, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LoadIconW,
    LoadImageW,
};

pub use patorrent_frontend::icons::Icon;
use patorrent_frontend::icons::{self, StateIcon, UiIcon};

/// The application icon's resource ID (`build.rs`).
const APP_ICON_ID: u16 = 1;

unsafe fn dib(size: usize, bgra: &[u8]) -> HBITMAP {
    unsafe {
        let mut bmi: BITMAPINFO = zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size as i32;
        bmi.bmiHeader.biHeight = -(size as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let bmp = CreateDIBSection(null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if !bmp.is_null() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
        }
        bmp
    }
}

/// Normal and disabled toolbar image lists, `size` pixels square. In a mirrored (right to
/// left) window they mirror their images back, so Start still points right and the RSS mark
/// keeps its shape.
pub fn toolbar_images(size: usize) -> (HIMAGELIST, HIMAGELIST) {
    let flags = ILC_COLOR32 | crate::util::rtl(ILC_MIRROR);
    unsafe {
        let normal = ImageList_Create(size as i32, size as i32, flags, Icon::ALL.len() as i32, 0);
        let disabled = ImageList_Create(size as i32, size as i32, flags, Icon::ALL.len() as i32, 0);
        for icon in Icon::ALL {
            for (list, gray) in [(normal, false), (disabled, true)] {
                let bmp = dib(size, &icons::toolbar_bgra(icon, size, gray));
                ImageList_Add(list, bmp, null_mut());
                DeleteObject(bmp);
            }
        }
        (normal, disabled)
    }
}

/// The application icon at `size` pixels (window, taskbar, tray, About box): the frame of the
/// embedded icon that fits best. Windows' generic icon when the executable has none (a
/// non-MSVC build, where `build.rs` embeds nothing). Free with `DestroyIcon`.
pub fn app_icon(size: usize) -> HICON {
    unsafe {
        let module = GetModuleHandleW(null());
        let s = size as i32;
        let icon = LoadImageW(module, APP_ICON_ID as usize as *const u16, IMAGE_ICON, s, s, LR_DEFAULTCOLOR);
        if icon.is_null() { LoadIconW(null_mut(), IDI_APPLICATION) } else { icon as HICON }
    }
}

/// The resource ID of the application icon, for `MessageBoxIndirectW`'s `MB_USERICON`.
pub fn app_icon_resource() -> *const u16 {
    APP_ICON_ID as usize as *const u16
}

/// A state icon at [`icons::state_size`]`(size)` pixels, from its PNG. Free with `DestroyIcon`.
pub fn state_icon(icon: StateIcon, size: usize) -> HICON {
    let size = icons::state_size(size as u32);
    png_icon(icon.png(size), size)
}

/// An icon from a PNG of `size` pixels. Free with `DestroyIcon`.
fn png_icon(png: &[u8], size: u32) -> HICON {
    // 0x00030000: the icon format version every image since Windows 3 uses, PNG included
    unsafe {
        CreateIconFromResourceEx(
            png.as_ptr(),
            png.len() as u32,
            1,
            0x0003_0000,
            size as i32,
            size as i32,
            LR_DEFAULTCOLOR,
        )
    }
}

/// Every state icon in [`StateIcon::ALL`] order (`icon as i32` is its index), at the size for
/// `size` pixels; returns the list and its image size. Shared by the torrent list and the
/// Trackers tab (both `LVS_SHAREIMAGELISTS`): the caller destroys it.
pub fn state_images(size: usize) -> (HIMAGELIST, usize) {
    let px = icons::state_size(size as u32) as i32;
    unsafe {
        // mirrored back in a right-to-left window, as the toolbar's are
        let flags = ILC_COLOR32 | crate::util::rtl(ILC_MIRROR);
        let list = ImageList_Create(px, px, flags, StateIcon::ALL.len() as i32, 0);
        for icon in StateIcon::ALL {
            let h = state_icon(icon, px as usize);
            ImageList_ReplaceIcon(list, -1, h);
            DestroyIcon(h);
        }
        (list, px as usize)
    }
}

/// Every navigation symbol in [`UiIcon::ALL`] order (`icon as i32` is its index), at the size
/// for `size` pixels: the category list's and the detail tabs' (one list, shared), and the
/// Preferences tree's. The caller destroys it.
pub fn ui_images(size: usize) -> HIMAGELIST {
    let px = icons::state_size(size as u32);
    unsafe {
        let flags = ILC_COLOR32 | crate::util::rtl(ILC_MIRROR);
        let list = ImageList_Create(px as i32, px as i32, flags, UiIcon::ALL.len() as i32, 0);
        for icon in UiIcon::ALL {
            let h = png_icon(icon.png(px), px);
            ImageList_ReplaceIcon(list, -1, h);
            DestroyIcon(h);
        }
        list
    }
}
