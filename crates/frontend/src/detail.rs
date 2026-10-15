//! The detail tabs' tables (Trackers, Peers, Files) in µTorrent 2.2.1's layout: column titles,
//! default widths and cell text.

use patorrent_core::{
    AnnounceEntry, DownloadPriority, FileEntry, PeerFlags, PeerInfo, PeerSource, TrackerStatus,
};

use crate::fmt;

/// Title, width at 96 DPI, right-aligned.
pub type ColumnDef = (&'static str, i32, bool);

pub const TRACKER_COLUMNS: [ColumnDef; 6] = [
    ("Name", 300, false),
    ("Status", 200, false),
    ("Update In", 80, true),
    ("Seeds", 60, true),
    ("Peers", 60, true),
    ("Downloaded", 80, true),
];

pub fn tracker_cell(t: &AnnounceEntry, col: usize) -> String {
    let opt = |v: Option<u32>| v.map(|v| v.to_string()).unwrap_or_default();
    match col {
        0 => t.url.clone(),
        1 => tracker_status(t),
        2 => t.next_announce.map(|s| fmt::duration(s as u64)).unwrap_or_default(),
        3 => opt(t.scrape_complete),
        4 => opt(t.scrape_incomplete),
        5 => opt(t.scrape_downloaded),
        _ => String::new(),
    }
}

pub fn tracker_status(t: &AnnounceEntry) -> String {
    match t.status {
        TrackerStatus::NotContacted => String::new(),
        TrackerStatus::Working if t.message.is_empty() => "Working".to_owned(),
        TrackerStatus::Working => format!("Working: {}", t.message),
        TrackerStatus::Updating => "Updating...".to_owned(),
        TrackerStatus::Error if t.message.is_empty() => "Error".to_owned(),
        TrackerStatus::Error => t.message.clone(),
    }
}

pub const PEER_COLUMNS: [ColumnDef; 9] = [
    ("IP", 160, false),
    ("Client", 130, false),
    ("Flags", 60, false),
    ("%", 50, true),
    ("Down Speed", 75, true),
    ("Up Speed", 75, true),
    ("Reqs", 50, true),
    ("Downloaded", 75, true),
    ("Uploaded", 75, true),
];

pub fn peer_cell(p: &PeerInfo, col: usize) -> String {
    match col {
        0 => p.ip.to_string(),
        1 => p.client.clone(),
        2 => peer_flags(p),
        3 => format!("{:.1}", p.progress_ppm as f64 / 10_000.0),
        4 => fmt::speed(p.down_speed),
        5 => fmt::speed(p.up_speed),
        6 => format!("{} | {}", p.download_queue_length, p.upload_queue_length),
        7 => fmt::size(p.total_download),
        8 => fmt::size(p.total_upload),
        _ => String::new(),
    }
}

/// µTorrent's peer flags: `D`/`d` downloading from them (unchoked / choked), `U`/`u` uploading
/// to them (unchoked / choked), `O` optimistic unchoke, `S` snubbed, `I` incoming, `E`/`e`
/// encrypted (RC4 / header only), `P` uTP, `H` DHT, `X` PEX, `L` local discovery.
pub fn peer_flags(p: &PeerInfo) -> String {
    let f = p.flags;
    let mut s = String::new();
    if f.contains(PeerFlags::INTERESTING) {
        s.push(if f.contains(PeerFlags::REMOTE_CHOKED) { 'd' } else { 'D' });
    }
    if f.contains(PeerFlags::REMOTE_INTERESTED) {
        s.push(if f.contains(PeerFlags::CHOKED) { 'u' } else { 'U' });
    }
    if f.contains(PeerFlags::OPTIMISTIC_UNCHOKE) {
        s.push('O');
    }
    if f.contains(PeerFlags::SNUBBED) {
        s.push('S');
    }
    if p.source.contains(PeerSource::INCOMING) {
        s.push('I');
    }
    if f.contains(PeerFlags::RC4_ENCRYPTED) {
        s.push('E');
    } else if f.contains(PeerFlags::PLAINTEXT_ENCRYPTED) {
        s.push('e');
    }
    if f.contains(PeerFlags::UTP_SOCKET) {
        s.push('P');
    }
    if p.source.contains(PeerSource::DHT) {
        s.push('H');
    }
    if p.source.contains(PeerSource::PEX) {
        s.push('X');
    }
    if p.source.contains(PeerSource::LSD) {
        s.push('L');
    }
    s
}

pub const FILE_COLUMNS: [ColumnDef; 7] = [
    ("Name", 300, false),
    ("Size", 75, true),
    ("Done", 75, true),
    ("%", 50, true),
    ("# Pieces", 60, true),
    ("First Piece", 70, true),
    ("Priority", 90, false),
];

/// `done` is the file's completed bytes, if known.
pub fn file_cell(f: &FileEntry, done: Option<u64>, col: usize) -> String {
    match col {
        0 => f.path.clone(),
        1 => fmt::size(f.size),
        2 => done.map(fmt::size).unwrap_or_default(),
        3 => match done {
            Some(d) if f.size > 0 => format!("{:.1} %", (d as f64 / f.size as f64 * 1000.0).floor() / 10.0),
            Some(_) => "100.0 %".to_owned(),
            None => String::new(),
        },
        4 => (f.last_piece - f.first_piece + 1).to_string(),
        5 => f.first_piece.to_string(),
        6 => priority_text(f.priority).to_owned(),
        _ => String::new(),
    }
}

/// µTorrent's three file priorities plus "skip".
pub fn priority_text(p: DownloadPriority) -> &'static str {
    match p.0 {
        0 => "Don't Download",
        1..=3 => "Low",
        4 => "Normal",
        _ => "High",
    }
}

/// The priorities the Files tab's menu offers, in menu order.
pub const PRIORITIES: [(&str, DownloadPriority); 4] = [
    ("High Priority", DownloadPriority::TOP),
    ("Normal Priority", DownloadPriority::NORMAL),
    ("Low Priority", DownloadPriority::LOW),
    ("Don't Download", DownloadPriority::DONT_DOWNLOAD),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(flags: PeerFlags, source: PeerSource) -> PeerInfo {
        PeerInfo {
            ip: "10.0.0.1:6881".parse().unwrap(),
            client: "qBittorrent 5.2.4".into(),
            flags,
            source,
            progress_ppm: 500_000,
            down_speed: 0,
            up_speed: 0,
            total_download: 0,
            total_upload: 0,
            download_queue_length: 3,
            upload_queue_length: 0,
            num_hashfails: 0,
            is_web_seed: false,
        }
    }

    #[test]
    fn peer_flag_letters() {
        let p = peer(
            PeerFlags::INTERESTING | PeerFlags::REMOTE_INTERESTED | PeerFlags::CHOKED,
            PeerSource::INCOMING,
        );
        assert_eq!(peer_flags(&p), "DuI");
        let p = peer(
            PeerFlags::INTERESTING | PeerFlags::REMOTE_CHOKED | PeerFlags::RC4_ENCRYPTED,
            PeerSource::TRACKER,
        );
        assert_eq!(peer_flags(&p), "dE");
        assert_eq!(peer_cell(&p, 3), "50.0");
        assert_eq!(peer_cell(&p, 6), "3 | 0");
    }

    #[test]
    fn trackers_and_files() {
        let mut t = AnnounceEntry { url: "udp://t.example:1/announce".into(), ..Default::default() };
        assert_eq!(tracker_cell(&t, 1), "");
        t.status = TrackerStatus::Error;
        t.message = "timed out".into();
        assert_eq!(tracker_cell(&t, 1), "timed out");
        t.next_announce = Some(125);
        assert_eq!(tracker_cell(&t, 2), "2m 5s");

        let f = FileEntry {
            path: "a/b.iso".into(),
            size: 1000,
            offset: 0,
            first_piece: 2,
            last_piece: 5,
            pad_file: false,
            priority: DownloadPriority::NORMAL,
        };
        assert_eq!(file_cell(&f, Some(999), 3), "99.9 %");
        assert_eq!(file_cell(&f, None, 2), "");
        assert_eq!(file_cell(&f, None, 4), "4");
        assert_eq!(file_cell(&f, None, 6), "Normal");
        assert_eq!(priority_text(DownloadPriority::TOP), "High");
    }
}
