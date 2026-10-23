//! The torrent list's columns, as in µTorrent 2.2.1: titles, default widths and visibility, cell
//! text and sort order. Frontends only draw them.

use std::cmp::Ordering;

use patorrent_core::TorrentStatus;

use crate::status::status_text;
use crate::{fmt, tr};

/// Per-row data the engine does not know about.
#[derive(Clone, Copy, Debug, Default)]
pub struct RowExtra<'a> {
    /// The user chose Pause rather than Stop (see [`crate::status`]).
    pub user_paused: bool,
    pub label: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Column {
    Name,
    QueuePosition,
    Size,
    Done,
    Status,
    Seeds,
    Peers,
    DownSpeed,
    UpSpeed,
    Eta,
    Uploaded,
    Downloaded,
    Ratio,
    Availability,
    Label,
    AddedOn,
    CompletedOn,
    Tracker,
    DownLimit,
    UpLimit,
    Remaining,
    SeedsPeers,
    SaveAs,
    Wasted,
}

/// Cell alignment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Right,
}

impl Column {
    /// In µTorrent 2.2.1's default order. This is also the stable identity order used when
    /// saving the layout, so append new columns at the end.
    pub const ALL: [Column; 24] = [
        Column::Name,
        Column::QueuePosition,
        Column::Size,
        Column::Done,
        Column::Status,
        Column::Seeds,
        Column::Peers,
        Column::DownSpeed,
        Column::UpSpeed,
        Column::Eta,
        Column::Uploaded,
        Column::Downloaded,
        Column::Ratio,
        Column::Availability,
        Column::Label,
        Column::AddedOn,
        Column::CompletedOn,
        Column::Tracker,
        Column::DownLimit,
        Column::UpLimit,
        Column::Remaining,
        Column::SeedsPeers,
        Column::SaveAs,
        Column::Wasted,
    ];

    /// Short stable key for saved layouts.
    pub fn key(self) -> &'static str {
        match self {
            Column::Name => "name",
            Column::QueuePosition => "queue",
            Column::Size => "size",
            Column::Done => "done",
            Column::Status => "status",
            Column::Seeds => "seeds",
            Column::Peers => "peers",
            Column::DownSpeed => "dlspeed",
            Column::UpSpeed => "ulspeed",
            Column::Eta => "eta",
            Column::Uploaded => "uploaded",
            Column::Downloaded => "downloaded",
            Column::Ratio => "ratio",
            Column::Availability => "avail",
            Column::Label => "label",
            Column::AddedOn => "added",
            Column::CompletedOn => "completed",
            Column::Tracker => "tracker",
            Column::DownLimit => "dllimit",
            Column::UpLimit => "ullimit",
            Column::Remaining => "remaining",
            Column::SeedsPeers => "seedspeers",
            Column::SaveAs => "saveas",
            Column::Wasted => "wasted",
        }
    }

    pub fn from_key(key: &str) -> Option<Column> {
        Column::ALL.into_iter().find(|c| c.key() == key)
    }

    pub fn title(self) -> &'static str {
        match self {
            Column::Name => tr!(ColName),
            Column::QueuePosition => tr!(ColQueue),
            Column::Size => tr!(ColSize),
            Column::Done => tr!(ColDone),
            Column::Status => tr!(ColStatus),
            Column::Seeds => tr!(ColSeeds),
            Column::Peers => tr!(ColPeers),
            Column::DownSpeed => tr!(ColDownSpeed),
            Column::UpSpeed => tr!(ColUpSpeed),
            Column::Eta => tr!(ColEta),
            Column::Uploaded => tr!(ColUploaded),
            Column::Downloaded => tr!(ColDownloaded),
            Column::Ratio => tr!(ColRatio),
            Column::Availability => tr!(ColAvailability),
            Column::Label => tr!(ColLabel),
            Column::AddedOn => tr!(ColAddedOn),
            Column::CompletedOn => tr!(ColCompletedOn),
            Column::Tracker => tr!(ColTracker),
            Column::DownLimit => tr!(ColDownLimit),
            Column::UpLimit => tr!(ColUpLimit),
            Column::Remaining => tr!(ColRemaining),
            Column::SeedsPeers => tr!(ColSeedsPeers),
            Column::SaveAs => tr!(ColSaveAs),
            Column::Wasted => tr!(ColWasted),
        }
    }

    /// Width in 96-DPI pixels.
    pub fn default_width(self) -> i32 {
        match self {
            Column::Name => 220,
            Column::QueuePosition => 28,
            Column::Done => 90,
            Column::Status => 110,
            Column::AddedOn | Column::CompletedOn => 125,
            Column::Tracker | Column::SaveAs => 180,
            _ => 70,
        }
    }

    pub fn default_visible(self) -> bool {
        matches!(
            self,
            Column::Name
                | Column::QueuePosition
                | Column::Size
                | Column::Done
                | Column::Status
                | Column::Seeds
                | Column::Peers
                | Column::DownSpeed
                | Column::UpSpeed
                | Column::Eta
                | Column::Uploaded
                | Column::Ratio
                | Column::Availability
                | Column::Label
        )
    }

    pub fn align(self) -> Align {
        match self {
            Column::Name
            | Column::Status
            | Column::Label
            | Column::Tracker
            | Column::SaveAs
            | Column::Done
            | Column::AddedOn
            | Column::CompletedOn => Align::Left,
            _ => Align::Right,
        }
    }

    /// Descending first for "more is interesting" columns, as µTorrent does on the first click.
    pub fn default_descending(self) -> bool {
        matches!(
            self,
            Column::DownSpeed
                | Column::UpSpeed
                | Column::Seeds
                | Column::Peers
                | Column::Ratio
                | Column::Done
        )
    }

    /// The cell text.
    pub fn text(self, st: &TorrentStatus, x: RowExtra<'_>) -> String {
        match self {
            Column::Name => st.name.clone(),
            Column::QueuePosition => {
                st.queue_position.map_or_else(|| "*".to_owned(), |q| (q + 1).to_string())
            }
            Column::Size => fmt::size(st.total_wanted),
            Column::Done => fmt::percent(st.progress_ppm),
            Column::Status => status_text(st, x.user_paused),
            Column::Seeds => swarm(st.num_seeds, st.num_complete),
            Column::Peers => swarm(st.num_peers.saturating_sub(st.num_seeds), st.num_incomplete),
            Column::DownSpeed => fmt::speed(st.download_payload_rate),
            Column::UpSpeed => fmt::speed(st.upload_payload_rate),
            Column::Eta => {
                if st.is_finished || st.flags.contains(patorrent_core::TorrentFlags::PAUSED) {
                    String::new()
                } else {
                    fmt::eta(st.eta())
                }
            }
            Column::Uploaded => fmt::size(st.all_time_upload),
            Column::Downloaded => fmt::size(st.all_time_download),
            Column::Ratio => fmt::ratio(st.ratio()),
            Column::Availability => fmt::availability(st.distributed_copies),
            Column::Label => x.label.to_owned(),
            Column::AddedOn => fmt::date_time(st.added_time),
            Column::CompletedOn => fmt::date_time(st.completed_time),
            Column::Tracker => host(&st.current_tracker).to_owned(),
            Column::DownLimit => fmt::speed(st.download_limit),
            Column::UpLimit => fmt::speed(st.upload_limit),
            Column::Remaining => fmt::size(st.total_wanted.saturating_sub(st.total_wanted_done)),
            Column::SeedsPeers => match seeds_peers(st) {
                Some(r) if r.is_infinite() => "∞".to_owned(),
                Some(r) => format!("{r:.3}"),
                None => String::new(),
            },
            Column::SaveAs => st.save_path.clone(),
            Column::Wasted => fmt::size(st.total_failed_bytes + st.total_redundant_bytes),
        }
    }

    /// Ascending order of two rows by this column. Ties are left to the caller.
    pub fn cmp(self, a: &TorrentStatus, b: &TorrentStatus, xa: RowExtra<'_>, xb: RowExtra<'_>) -> Ordering {
        match self {
            Column::Name => natural_cmp(&a.name, &b.name),
            // Torrents outside the queue sort after queued ones.
            Column::QueuePosition => {
                a.queue_position.unwrap_or(u32::MAX).cmp(&b.queue_position.unwrap_or(u32::MAX))
            }
            Column::Size => a.total_wanted.cmp(&b.total_wanted),
            Column::Done => a.progress_ppm.cmp(&b.progress_ppm),
            Column::Status => status_text(a, xa.user_paused).cmp(&status_text(b, xb.user_paused)),
            Column::Seeds => (a.num_seeds, a.num_complete).cmp(&(b.num_seeds, b.num_complete)),
            Column::Peers => (a.num_peers - a.num_seeds.min(a.num_peers), a.num_incomplete)
                .cmp(&(b.num_peers - b.num_seeds.min(b.num_peers), b.num_incomplete)),
            Column::DownSpeed => a.download_payload_rate.cmp(&b.download_payload_rate),
            Column::UpSpeed => a.upload_payload_rate.cmp(&b.upload_payload_rate),
            // Unknown ETA sorts last.
            Column::Eta => a.eta().unwrap_or(u64::MAX).cmp(&b.eta().unwrap_or(u64::MAX)),
            Column::Uploaded => a.all_time_upload.cmp(&b.all_time_upload),
            Column::Downloaded => a.all_time_download.cmp(&b.all_time_download),
            Column::Ratio => a.ratio().total_cmp(&b.ratio()),
            Column::Availability => {
                a.distributed_copies.unwrap_or(-1.0).total_cmp(&b.distributed_copies.unwrap_or(-1.0))
            }
            Column::Label => natural_cmp(xa.label, xb.label),
            Column::AddedOn => a.added_time.cmp(&b.added_time),
            Column::CompletedOn => a.completed_time.cmp(&b.completed_time),
            Column::Tracker => host(&a.current_tracker).cmp(host(&b.current_tracker)),
            Column::DownLimit => a.download_limit.cmp(&b.download_limit),
            Column::UpLimit => a.upload_limit.cmp(&b.upload_limit),
            Column::Remaining => a
                .total_wanted
                .saturating_sub(a.total_wanted_done)
                .cmp(&b.total_wanted.saturating_sub(b.total_wanted_done)),
            Column::SeedsPeers => seeds_peers(a).unwrap_or(-1.0).total_cmp(&seeds_peers(b).unwrap_or(-1.0)),
            Column::SaveAs => natural_cmp(&a.save_path, &b.save_path),
            Column::Wasted => (a.total_failed_bytes + a.total_redundant_bytes)
                .cmp(&(b.total_failed_bytes + b.total_redundant_bytes)),
        }
    }
}

/// `3 (120)`: connected, then the swarm total from the tracker when known.
fn swarm(connected: u32, total: Option<u32>) -> String {
    match total {
        Some(t) => format!("{connected} ({t})"),
        None => connected.to_string(),
    }
}

/// Seeds per peer in the swarm (scrape counts when known, else connected ones).
fn seeds_peers(st: &TorrentStatus) -> Option<f64> {
    let seeds = st.num_complete.unwrap_or(st.num_seeds);
    let peers = st.num_incomplete.unwrap_or(st.num_peers.saturating_sub(st.num_seeds));
    match (seeds, peers) {
        (0, 0) => None,
        (_, 0) => Some(f64::INFINITY),
        (s, p) => Some(s as f64 / p as f64),
    }
}

/// `tracker.example.org` from `udp://tracker.example.org:6969/announce`.
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if authority.starts_with('[') {
        return authority.split_once(']').map_or(authority, |(h, _)| &h[1..]);
    }
    authority.split_once(':').map_or(authority, |(h, _)| h)
}

/// Case-insensitive comparison where digit runs compare by value ("ep2" < "ep10"), like
/// Explorer's `StrCmpLogicalW`. ASCII is compared byte by byte without allocating (sorting
/// 100,000 names calls this about two million times); other characters by their lowercase forms.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    loop {
        let (x, y) = match (ab.get(i), bb.get(j)) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&x), Some(&y)) => (x, y),
        };
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let ei = i + ab[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            let ej = j + bb[j..].iter().take_while(|c| c.is_ascii_digit()).count();
            let zeros = |s: &[u8]| s.iter().take_while(|&&c| c == b'0').count();
            let (ta, tb) = (&ab[i + zeros(&ab[i..ei])..ei], &bb[j + zeros(&bb[j..ej])..ej]);
            let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
            if o != Ordering::Equal {
                return o;
            }
            (i, j) = (ei, ej);
        } else if x.is_ascii() && y.is_ascii() {
            let o = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
            if o != Ordering::Equal {
                return o;
            }
            (i, j) = (i + 1, j + 1);
        } else {
            // `i` and `j` stay on character boundaries: they only move by whole characters
            let (Some(ca), Some(cb)) = (a[i..].chars().next(), b[j..].chars().next()) else {
                return Ordering::Equal;
            };
            let o = ca.to_lowercase().cmp(cb.to_lowercase());
            if o != Ordering::Equal {
                return o;
            }
            (i, j) = (i + ca.len_utf8(), j + cb.len_utf8());
        }
    }
}

/// A byte string whose plain comparison orders names as [`natural_cmp`] does, made once per
/// name so that sorting 100,000 names compares bytes rather than parsing each name about 17
/// times. Digit runs become `'0'`, the length without leading zeros (4 bytes, big-endian) and
/// those digits; other characters their lowercase form in UTF-8. `None` for a name with a
/// character whose lowercase is several characters (such as `İ`), where the two orders could
/// differ: sort those with [`natural_cmp`].
pub fn natural_key(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() + 8);
    natural_key_into(s, &mut out).then_some(out)
}

/// [`natural_key`] appended to `out` (one buffer for many names); false, with `out` as it was,
/// where `natural_key` gives `None`.
pub fn natural_key_into(s: &str, out: &mut Vec<u8>) -> bool {
    let len = out.len();
    let ok = push_natural_key(s, out).is_some();
    if !ok {
        out.truncate(len);
    }
    ok
}

fn push_natural_key(s: &str, out: &mut Vec<u8>) -> Option<()> {
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c.is_ascii_digit() {
            let end = i + b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            let start = i + b[i..end].iter().take_while(|&&c| c == b'0').count();
            // a digit compares with any other character as '0' does: all ten sit between the
            // same neighbours
            out.push(b'0');
            out.extend_from_slice(&u32::try_from(end - start).ok()?.to_be_bytes());
            out.extend_from_slice(&b[start..end]);
            i = end;
        } else if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
            i += 1;
        } else {
            let ch = s[i..].chars().next()?;
            let mut lower = ch.to_lowercase();
            let l = lower.next()?;
            if lower.next().is_some() {
                return None;
            }
            out.extend_from_slice(l.encode_utf8(&mut [0; 4]).as_bytes());
            i += ch.len_utf8();
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_are_unique() {
        for c in Column::ALL {
            assert_eq!(Column::from_key(c.key()), Some(c));
            assert_eq!(Column::ALL.iter().filter(|d| d.key() == c.key()).count(), 1);
        }
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("ep2", "ep10"), Ordering::Less);
        assert_eq!(natural_cmp("Ubuntu", "debian"), Ordering::Greater);
        assert_eq!(natural_cmp("a01", "a1"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
    }

    /// The comparison before it stopped allocating: chars, digit runs collected into strings.
    fn reference_cmp(a: &str, b: &str) -> Ordering {
        fn digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
            let mut s = String::new();
            while let Some(c) = it.next_if(|c| c.is_ascii_digit()) {
                s.push(c);
            }
            s
        }
        let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
        loop {
            match (a.peek().copied(), b.peek().copied()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                    let (na, nb) = (digits(&mut a), digits(&mut b));
                    let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                    let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                (Some(x), Some(y)) => {
                    let o = x.to_lowercase().cmp(y.to_lowercase());
                    if o != Ordering::Equal {
                        return o;
                    }
                    a.next();
                    b.next();
                }
            }
        }
    }

    #[test]
    fn natural_order_matches_the_char_by_char_reference() {
        let words = [
            "",
            "a",
            "A",
            "ab",
            "aB1",
            "ep2",
            "ep10",
            "ep010",
            "Ep 9",
            "x00",
            "x0",
            "x",
            "0",
            "007",
            "7",
            "\u{dc}n\u{ef}code",
            "\u{fc}nicode 2",
            "\u{fc}ber",
            "Zebra",
            "\u{df}",
            "SS",
            "\u{130}stanbul",
            "istanbul",
            "\u{65e5}\u{672c} 1",
            "\u{65e5}\u{672c} 01",
            "a1b2",
            "a01b02",
            "a1b10",
            "-1",
            "_x",
            "[x]",
            "a\u{0}b",
            "\u{3a9}",
            "\u{3c9}2",
            "\u{3c9}10",
            "9z",
            "99",
            "100",
        ];
        for a in words {
            for b in words {
                assert_eq!(natural_cmp(a, b), reference_cmp(a, b), "{a:?} vs {b:?}");
                if let (Some(ka), Some(kb)) = (natural_key(a), natural_key(b)) {
                    assert_eq!(ka.cmp(&kb), reference_cmp(a, b), "keys of {a:?} vs {b:?}");
                }
            }
        }
        assert!(natural_key("\u{130}stanbul").is_none(), "İ lowercases to two characters");
    }

    #[test]
    fn natural_keys_order_random_names_as_natural_cmp() {
        // characters around the digits, upper and lower case, a few non-ASCII ones
        let alphabet: Vec<char> = "0012789 !/:A_aZz.-\u{e9}\u{c9}\u{212a}k\u{3a3}\u{3c3}".chars().collect();
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % n as u64) as usize
        };
        let names: Vec<String> =
            (0..400).map(|_| (0..next(7)).map(|_| alphabet[next(alphabet.len())]).collect()).collect();
        for a in &names {
            for b in &names {
                let (ka, kb) = (natural_key(a).unwrap(), natural_key(b).unwrap());
                assert_eq!(ka.cmp(&kb), natural_cmp(a, b), "{a:?} vs {b:?}");
                assert_eq!(natural_cmp(a, b), reference_cmp(a, b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn hosts() {
        assert_eq!(host("udp://tracker.example.org:6969/announce"), "tracker.example.org");
        assert_eq!(host("https://u:p@t.example/a?x=1"), "t.example");
        assert_eq!(host("http://[2001:db8::1]:80/announce"), "2001:db8::1");
        assert_eq!(host(""), "");
    }

    #[test]
    fn cells() {
        let st = TorrentStatus {
            num_seeds: 2,
            num_peers: 5,
            num_complete: Some(40),
            queue_position: Some(0),
            ..TorrentStatus::default()
        };
        let x = RowExtra::default();
        assert_eq!(Column::Seeds.text(&st, x), "2 (40)");
        assert_eq!(Column::Peers.text(&st, x), "3");
        assert_eq!(Column::QueuePosition.text(&st, x), "1");
        assert_eq!(Column::DownSpeed.text(&st, x), "");
    }
}
