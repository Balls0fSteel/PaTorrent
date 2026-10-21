//! The toolbar, window and tray icons as Windows bitmaps, at the window's DPI, from the shared
//! drawings in `patorrent_frontend::icons` (no image files, no resource compiler).

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
    HBITMAP,
};
use windows_sys::Win32::UI::Controls::{
    HIMAGELIST, ILC_COLOR32, ILC_MIRROR, ImageList_Add, ImageList_Create,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

use patorrent_frontend::icons;
pub use patorrent_frontend::icons::Icon;

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

/// The application icon at `size` pixels (window, taskbar, tray).
pub fn app_icon(size: usize) -> HICON {
    unsafe {
        let color = dib(size, &icons::app_bgra(size));
        // 32-bit icons take their alpha from the color bitmap; the mask is still required.
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, null_mut());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}
