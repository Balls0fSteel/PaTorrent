//! The torrent list's columns, as in µTorrent 2.2.1: titles, default widths and visibility, cell
//! text and sort order. Frontends only draw them.

use std::cmp::Ordering;

use libretorrent_core::TorrentStatus;

use crate::fmt;
use crate::status::status_text;

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
            Column::Name => "Name",
            Column::QueuePosition => "#",
            Column::Size => "Size",
            Column::Done => "Done",
            Column::Status => "Status",
            Column::Seeds => "Seeds",
            Column::Peers => "Peers",
            Column::DownSpeed => "Down Speed",
            Column::UpSpeed => "Up Speed",
            Column::Eta => "ETA",
            Column::Uploaded => "Uploaded",
            Column::Downloaded => "Downloaded",
            Column::Ratio => "Ratio",
            Column::Availability => "Avail.",
            Column::Label => "Label",
            Column::AddedOn => "Added On",
            Column::CompletedOn => "Completed On",
            Column::Tracker => "Tracker",
            Column::DownLimit => "Down Limit",
            Column::UpLimit => "Up Limit",
            Column::Remaining => "Remaining",
            Column::SeedsPeers => "Seeds/Peers",
            Column::SaveAs => "Save As",
            Column::Wasted => "Wasted",
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
                if st.is_finished || st.flags.contains(libretorrent_core::TorrentFlags::PAUSED) {
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
/// Explorer's `StrCmpLogicalW`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na = digits(&mut a);
                let nb = digits(&mut b);
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

fn digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(c) = it.next_if(|c| c.is_ascii_digit()) {
        s.push(c);
    }
    s
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
