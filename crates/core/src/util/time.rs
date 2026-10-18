//! The local time zone's offset from UTC, for frontends that show or act on wall-clock time
//! (the weekly scheduler, dates in lists). Asked from the OS each call, so a DST change is seen.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since 1970-01-01 UTC.
pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Seconds east of UTC at `unix`; 0 where the OS does not say.
#[cfg(unix)]
pub fn local_utc_offset(unix: i64) -> i32 {
    // `struct tm` on Linux (glibc, musl), the BSDs and macOS: nine ints, then `tm_gmtoff`.
    #[repr(C)]
    struct Tm {
        ints: [std::ffi::c_int; 9],
        gmtoff: std::ffi::c_long,
        zone: *const std::ffi::c_char,
    }
    unsafe extern "C" {
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let mut tm = Tm { ints: [0; 9], gmtoff: 0, zone: std::ptr::null() };
    // SAFETY: both pointers are valid for the call; `time_t` is 64-bit on every target we build.
    let ok = unsafe { !localtime_r(&unix, &mut tm).is_null() };
    if ok { tm.gmtoff as i32 } else { 0 }
}

/// Seconds east of UTC now (`unix` is ignored: Windows is asked for the current offset).
#[cfg(windows)]
pub fn local_utc_offset(_unix: i64) -> i32 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    use windows_sys::Win32::System::Time::SystemTimeToFileTime;
    // SAFETY: plain out-parameters of the right types.
    unsafe {
        let (mut st_l, mut st_u) = (std::mem::zeroed(), std::mem::zeroed());
        GetLocalTime(&mut st_l);
        GetSystemTime(&mut st_u);
        let (mut fl, mut fu): (FILETIME, FILETIME) = (std::mem::zeroed(), std::mem::zeroed());
        SystemTimeToFileTime(&st_l, &mut fl);
        SystemTimeToFileTime(&st_u, &mut fu);
        let t = |f: FILETIME| ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64;
        // 100 ns units; round to whole minutes.
        (((t(fl) - t(fu)) / 10_000_000 + 30) / 60 * 60) as i32
    }
}

#[cfg(not(any(unix, windows)))]
pub fn local_utc_offset(_unix: i64) -> i32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offset_is_a_real_time_zone() {
        let off = local_utc_offset(unix_now());
        assert!((-12 * 3600..=14 * 3600).contains(&off), "{off}");
        assert_eq!(off % 60, 0);
        assert!(unix_now() > 1_700_000_000);
    }
}
