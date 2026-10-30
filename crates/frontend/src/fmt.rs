//! Number formatting in µTorrent's style: binary units with lower-case `kB`, three significant
//! digits for sizes, one decimal for speeds, two-unit ETAs and `∞` for unknown or never. Unit
//! names come from the string tables.

use crate::lang::{S, text};
use crate::tr;

const UNITS: [S; 6] = [S::UnitB, S::UnitKb, S::UnitMb, S::UnitGb, S::UnitTb, S::UnitPb];

/// `0 B`, `512 B`, `1.50 kB`, `12.3 MB`, `698 MB`, `4.37 GB`.
pub fn size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} {}", tr!(UnitB));
    }
    let mut v = bytes as f64;
    let mut unit = 0;
    // Switch units at 999.5 (not 1024) so the column never shows four integer digits.
    while v >= 999.5 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    let u = text(UNITS[unit]);
    if v >= 99.95 {
        format!("{v:.0} {u}")
    } else if v >= 9.995 {
        format!("{v:.1} {u}")
    } else {
        format!("{v:.2} {u}")
    }
}

/// `0.4 kB/s`, `312.0 kB/s`, `1.2 MB/s`. Zero is an empty string: µTorrent leaves idle speed cells
/// blank.
pub fn speed(bytes_per_sec: u32) -> String {
    if bytes_per_sec == 0 {
        return String::new();
    }
    speed_always(bytes_per_sec)
}

/// Like [`speed`] but renders zero as `0.0 kB/s` (status bar, General tab).
pub fn speed_always(bytes_per_sec: u32) -> String {
    let kb = bytes_per_sec as f64 / 1024.0;
    if kb < 1023.95 {
        format!("{kb:.1} {}", tr!(UnitKbPerSec))
    } else {
        format!("{:.1} {}", kb / 1024.0, tr!(UnitMbPerSec))
    }
}

/// The main window's title with "Show current speed in the title bar" (`ui.title_speed`):
/// `D:1.2 MB/s U:0.0 kB/s - PaTorrent 1.0.0`, µTorrent 2.2.1's `D:%s U:%s - %s`.
pub fn title_speed(down: u32, up: u32, title: &str) -> String {
    tr!(TitleSpeed, down = speed_always(down), up = speed_always(up), title)
}

/// Remaining time: `45s`, `12m 5s`, `3h 2m`, `2d 5h`, `3w 2d`; `∞` for unknown or a year and up.
pub fn eta(secs: Option<u64>) -> String {
    match secs {
        Some(s) if s < 365 * 86_400 => duration(s),
        _ => "∞".to_owned(),
    }
}

/// Elapsed time with the same two-unit layout as [`eta`].
pub fn duration(s: u64) -> String {
    const M: u64 = 60;
    const H: u64 = 60 * M;
    const D: u64 = 24 * H;
    const W: u64 = 7 * D;
    match s {
        _ if s < M => tr!(DurSeconds, s),
        _ if s < H => tr!(DurMinutes, m = s / M, s = s % M),
        _ if s < D => tr!(DurHours, h = s / H, m = (s % H) / M),
        _ if s < W => tr!(DurDays, d = s / D, h = (s % D) / H),
        _ => tr!(DurWeeks, w = s / W, d = (s % W) / D),
    }
}

/// Share ratio with three decimals; `∞` when uploaded with nothing downloaded.
pub fn ratio(r: f64) -> String {
    if r.is_infinite() { "∞".to_owned() } else { format!("{r:.3}") }
}

/// `0.0 %` .. `100.0 %` from parts per million. Never rounds up to 100 % before completion.
pub fn percent(ppm: u32) -> String {
    let tenths = (ppm.min(1_000_000) as u64 * 1000) / 1_000_000;
    format!("{}.{} %", tenths / 10, tenths % 10)
}

/// Swarm availability, as in the `Avail.` column: three decimals; blank if unknown.
pub fn availability(copies: Option<f32>) -> String {
    copies.map(|c| format!("{c:.3}")).unwrap_or_default()
}

static UTC_OFFSET: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Sets the local time zone's offset from UTC in seconds, used by [`date_time`]. Frontends set
/// it from the OS at startup (the frontend crate has no time-zone database).
pub fn set_utc_offset(secs: i32) {
    UTC_OFFSET.store(secs, std::sync::atomic::Ordering::Relaxed);
}

/// `2026-10-15 16:47:12` in local time; empty for 0 (never / unknown).
pub fn date_time(unix: i64) -> String {
    if unix <= 0 {
        return String::new();
    }
    let t = unix + UTC_OFFSET.load(std::sync::atomic::Ordering::Relaxed) as i64;
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date_time(0), "");
        assert_eq!(date_time(1_792_075_632), "2026-10-15 14:47:12");
        assert_eq!(date_time(951_782_400), "2000-02-29 00:00:00");
    }

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1536), "1.50 kB");
        assert_eq!(size(12_900), "12.6 kB");
        assert_eq!(size(698 << 20), "698 MB");
        assert_eq!(size(1023 << 10), "1.00 MB");
        assert_eq!(size((4.37 * (1u64 << 30) as f64) as u64), "4.37 GB");
        assert_eq!(size(u64::MAX), "16384 PB");
    }

    #[test]
    fn speeds() {
        assert_eq!(speed(0), "");
        assert_eq!(speed_always(0), "0.0 kB/s");
        assert_eq!(speed(400), "0.4 kB/s");
        assert_eq!(speed(312 * 1024), "312.0 kB/s");
        assert_eq!(speed(1258 * 1024), "1.2 MB/s");
        assert_eq!(title_speed(1258 * 1024, 0, "PaTorrent 1.0.0"), "D:1.2 MB/s U:0.0 kB/s - PaTorrent 1.0.0");
    }

    #[test]
    fn times() {
        assert_eq!(eta(None), "∞");
        assert_eq!(eta(Some(45)), "45s");
        assert_eq!(eta(Some(725)), "12m 5s");
        assert_eq!(eta(Some(3 * 3600 + 120)), "3h 2m");
        assert_eq!(eta(Some(2 * 86_400 + 5 * 3600)), "2d 5h");
        assert_eq!(eta(Some(23 * 86_400)), "3w 2d");
        assert_eq!(eta(Some(400 * 86_400)), "∞");
    }

    #[test]
    fn ratio_and_percent() {
        assert_eq!(ratio(0.12345), "0.123");
        assert_eq!(ratio(f64::INFINITY), "∞");
        assert_eq!(percent(0), "0.0 %");
        assert_eq!(percent(453_000), "45.3 %");
        assert_eq!(percent(999_999), "99.9 %");
        assert_eq!(percent(1_000_000), "100.0 %");
        assert_eq!(availability(Some(1.5)), "1.500");
        assert_eq!(availability(None), "");
    }
}
