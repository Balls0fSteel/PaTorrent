//! Number formatting in µTorrent's style: binary units with lower-case `kB`, three significant
//! digits for sizes, one decimal for speeds, two-unit ETAs and `∞` for unknown or never.

const UNITS: [&str; 6] = ["B", "kB", "MB", "GB", "TB", "PB"];

/// `0 B`, `512 B`, `1.50 kB`, `12.3 MB`, `698 MB`, `4.37 GB`.
pub fn size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut unit = 0;
    // Switch units at 999.5 (not 1024) so the column never shows four integer digits.
    while v >= 999.5 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    let u = UNITS[unit];
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
    if kb < 1023.95 { format!("{kb:.1} kB/s") } else { format!("{:.1} MB/s", kb / 1024.0) }
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
        _ if s < M => format!("{s}s"),
        _ if s < H => format!("{}m {}s", s / M, s % M),
        _ if s < D => format!("{}h {}m", s / H, (s % H) / M),
        _ if s < W => format!("{}d {}h", s / D, (s % D) / H),
        _ => format!("{}w {}d", s / W, (s % W) / D),
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

#[cfg(test)]
mod tests {
    use super::*;

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
