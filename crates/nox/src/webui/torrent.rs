//! Engine torrent status as WebAPI v2 torrent objects (`torrents/info`, `sync/maindata`):
//! field names, units and state strings as the qBittorrent WebAPI documentation gives them.

use std::path::Path;

use patorrent_core::{InfoHashes, TorrentFlags, TorrentState, TorrentStatus};
use patorrent_frontend::seeding::SeedGoal;

use super::json::Json;

/// The "infinite" ETA the API uses (100 days in seconds).
pub const MAX_ETA: i64 = 8_640_000;
/// Ratios are capped here, as an infinite ratio has no JSON number.
pub const MAX_RATIO: f64 = 9999.0;

/// The torrent's API hash: the v1 info-hash, or the truncated v2 hash of a v2-only torrent, in
/// lower-case hex.
pub fn hash_of(ih: &InfoHashes) -> String {
    ih.best_v1_compatible().to_hex()
}

/// What the WebAPI knows about a torrent besides its engine status.
#[derive(Clone, Debug, Default)]
pub struct Extra<'a> {
    pub category: &'a str,
    /// Unix seconds of the last payload transfer; 0 = never.
    pub last_activity: i64,
    pub private: bool,
    /// The torrent's own seeding goal, if it overrides the default.
    pub seed_goal: Option<SeedGoal>,
}

/// `ratio_limit` and `seeding_time_limit` (minutes) of a torrent's own goal: -2 without one
/// (the global limits apply), -1 for a target that is off.
pub fn share_limits(goal: Option<SeedGoal>) -> (f64, i64) {
    match goal {
        None => (-2.0, -2),
        Some(g) => (
            if g.ratio_pct == 0 { -1.0 } else { g.ratio_pct as f64 / 100.0 },
            if g.minutes == 0 { -1 } else { g.minutes as i64 },
        ),
    }
}

/// The documented `state` string.
pub fn state_str(st: &TorrentStatus) -> &'static str {
    let paused = st.flags.contains(TorrentFlags::PAUSED);
    let auto = st.flags.contains(TorrentFlags::AUTO_MANAGED);
    let up = st.is_finished;
    if !st.error.is_empty() {
        return "error";
    }
    if st.moving_storage {
        return "moving";
    }
    if st.state == TorrentState::CheckingResumeData {
        return "checkingResumeData";
    }
    if paused {
        return match (auto, up) {
            (true, true) => "queuedUP",
            (true, false) => "queuedDL",
            (false, true) => "pausedUP",
            (false, false) => "pausedDL",
        };
    }
    match st.state {
        TorrentState::CheckingFiles => {
            if up {
                "checkingUP"
            } else {
                "checkingDL"
            }
        }
        TorrentState::DownloadingMetadata => "metaDL",
        _ if up => match (auto, st.upload_payload_rate > 0) {
            (false, _) => "forcedUP",
            (true, true) => "uploading",
            (true, false) => "stalledUP",
        },
        _ => match (auto, st.download_payload_rate > 0) {
            (false, _) => "forcedDL",
            (true, true) => "downloading",
            (true, false) => "stalledDL",
        },
    }
}

/// `torrents/info` `filter` values. Unknown filters match everything.
pub fn matches_filter(filter: &str, st: &TorrentStatus) -> bool {
    let s = state_str(st);
    let active = st.download_payload_rate > 0 || st.upload_payload_rate > 0;
    let dl = s.ends_with("DL") || s == "downloading";
    let paused = s == "pausedUP" || s == "pausedDL";
    match filter {
        "downloading" => dl,
        "seeding" => matches!(s, "uploading" | "stalledUP" | "checkingUP" | "queuedUP" | "forcedUP"),
        "completed" => s.ends_with("UP") || s == "uploading",
        "paused" | "stopped" => paused,
        "resumed" | "running" => !paused,
        "active" => active,
        "inactive" => !active,
        "stalled" => s == "stalledUP" || s == "stalledDL",
        "stalled_uploading" => s == "stalledUP",
        "stalled_downloading" => s == "stalledDL",
        "checking" => s.starts_with("checking"),
        "moving" => s == "moving",
        "errored" => s == "error" || s == "missingFiles",
        _ => true,
    }
}

pub fn magnet_uri(st: &TorrentStatus) -> String {
    let mut m = String::from("magnet:?");
    let mut sep = "";
    if let Some(v1) = st.info_hashes.v1 {
        m.push_str("xt=urn:btih:");
        m.push_str(&v1.to_hex());
        sep = "&";
    }
    if let Some(v2) = st.info_hashes.v2 {
        m.push_str(sep);
        m.push_str("xt=urn:btmh:1220");
        m.push_str(&v2.to_hex());
    }
    if !st.name.is_empty() {
        m.push_str("&dn=");
        for b in st.name.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                m.push(b as char);
            } else {
                m.push_str(&format!("%{b:02X}"));
            }
        }
    }
    m
}

/// Progress of the wanted bytes (of the check while checking), 0..=1.
pub fn progress(st: &TorrentStatus) -> f64 {
    if matches!(st.state, TorrentState::CheckingFiles | TorrentState::CheckingResumeData) {
        return st.progress_ppm as f64 / 1e6;
    }
    if st.total_wanted == 0 {
        return if st.is_finished && st.has_metadata { 1.0 } else { 0.0 };
    }
    st.total_wanted_done as f64 / st.total_wanted as f64
}

pub fn ratio(st: &TorrentStatus) -> f64 {
    st.ratio().min(MAX_RATIO)
}

fn limit(bytes: u32) -> i64 {
    if bytes == 0 { -1 } else { bytes as i64 }
}

fn or_minus_one(v: Option<u32>) -> i64 {
    v.map_or(-1, i64::from)
}

/// Field names of a torrent object, in the order [`fields`] returns their values.
pub const FIELDS: [&str; 49] = [
    "added_on",
    "amount_left",
    "auto_tmm",
    "availability",
    "category",
    "completed",
    "completion_on",
    "content_path",
    "dl_limit",
    "dlspeed",
    "downloaded",
    "downloaded_session",
    "eta",
    "f_l_piece_prio",
    "force_start",
    "hash",
    "infohash_v1",
    "infohash_v2",
    "isPrivate",
    "last_activity",
    "magnet_uri",
    "max_ratio",
    "max_seeding_time",
    "name",
    "num_complete",
    "num_incomplete",
    "num_leechs",
    "num_seeds",
    "priority",
    "progress",
    "ratio",
    "ratio_limit",
    "reannounce",
    "save_path",
    "seeding_time",
    "seeding_time_limit",
    "seen_complete",
    "seq_dl",
    "size",
    "state",
    "super_seeding",
    "tags",
    "time_active",
    "total_size",
    "tracker",
    "up_limit",
    "uploaded",
    "uploaded_session",
    "upspeed",
];

/// The values of a torrent object in [`FIELDS`] order.
pub fn fields(st: &TorrentStatus, x: &Extra<'_>) -> Vec<Json> {
    let paused = st.flags.contains(TorrentFlags::PAUSED);
    let auto = st.flags.contains(TorrentFlags::AUTO_MANAGED);
    let content_path = if st.name.is_empty() {
        st.save_path.clone()
    } else {
        Path::new(&st.save_path).join(&st.name).to_string_lossy().into_owned()
    };
    let eta = st.eta().map_or(MAX_ETA, |e| (e as i64).min(MAX_ETA));
    let ratio = ratio(st);
    let (ratio_limit, seeding_time_limit) = share_limits(x.seed_goal);
    vec![
        Json::Int(st.added_time),
        st.total_wanted.saturating_sub(st.total_wanted_done).into(),
        false.into(),
        Json::Float(st.distributed_copies.map_or(-1.0, f64::from)),
        x.category.into(),
        st.total_wanted_done.into(),
        Json::Int(if st.completed_time > 0 { st.completed_time } else { -1 }),
        content_path.into(),
        Json::Int(limit(st.download_limit)),
        st.download_payload_rate.into(),
        st.all_time_download.into(),
        st.total_payload_download.into(),
        Json::Int(eta),
        false.into(),
        (!auto && !paused).into(),
        hash_of(&st.info_hashes).into(),
        st.info_hashes.v1.map(|h| h.to_hex()).unwrap_or_default().into(),
        st.info_hashes.v2.map(|h| h.to_hex()).unwrap_or_default().into(),
        x.private.into(),
        Json::Int(x.last_activity),
        magnet_uri(st).into(),
        Json::Float(-1.0),
        Json::Int(-1),
        st.name.clone().into(),
        Json::Int(or_minus_one(st.num_complete)),
        Json::Int(or_minus_one(st.num_incomplete)),
        st.num_peers.saturating_sub(st.num_seeds).into(),
        st.num_seeds.into(),
        Json::Int(st.queue_position.map_or(0, |p| p as i64 + 1)),
        Json::Float(progress(st)),
        Json::Float(ratio),
        Json::Float(ratio_limit),
        Json::Int(st.next_announce.map_or(0, i64::from)),
        st.save_path.clone().into(),
        st.seeding_duration.into(),
        Json::Int(seeding_time_limit),
        Json::Int(if st.last_seen_complete > 0 { st.last_seen_complete } else { -1 }),
        st.flags.contains(TorrentFlags::SEQUENTIAL_DOWNLOAD).into(),
        st.total_wanted.into(),
        state_str(st).into(),
        st.super_seeding.into(),
        "".into(),
        st.active_duration.into(),
        st.total.into(),
        st.current_tracker.clone().into(),
        Json::Int(limit(st.upload_limit)),
        st.all_time_upload.into(),
        st.total_payload_upload.into(),
        st.upload_payload_rate.into(),
    ]
}

/// A full torrent object.
pub fn object(st: &TorrentStatus, x: &Extra<'_>) -> Json {
    Json::Obj(FIELDS.iter().map(|f| f.to_string()).zip(fields(st, x)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use patorrent_core::Sha1Hash;

    fn st(flags: TorrentFlags, state: TorrentState, finished: bool) -> TorrentStatus {
        TorrentStatus { flags, state, is_finished: finished, ..TorrentStatus::default() }
    }

    #[test]
    fn states() {
        let auto = TorrentFlags::AUTO_MANAGED;
        let paused = TorrentFlags::PAUSED;
        let dl = TorrentState::Downloading;
        assert_eq!(state_str(&st(paused, dl, false)), "pausedDL");
        assert_eq!(state_str(&st(paused, TorrentState::Seeding, true)), "pausedUP");
        assert_eq!(state_str(&st(paused | auto, dl, false)), "queuedDL");
        assert_eq!(state_str(&st(paused | auto, dl, true)), "queuedUP");
        assert_eq!(state_str(&st(auto, dl, false)), "stalledDL");
        assert_eq!(state_str(&st(TorrentFlags::empty(), dl, false)), "forcedDL");
        assert_eq!(state_str(&st(TorrentFlags::empty(), TorrentState::Seeding, true)), "forcedUP");
        assert_eq!(state_str(&st(auto, TorrentState::DownloadingMetadata, false)), "metaDL");
        assert_eq!(state_str(&st(auto, TorrentState::CheckingFiles, false)), "checkingDL");
        assert_eq!(state_str(&st(auto, TorrentState::CheckingResumeData, false)), "checkingResumeData");
        let mut s = st(auto, dl, false);
        s.download_payload_rate = 5;
        assert_eq!(state_str(&s), "downloading");
        s.is_finished = true;
        s.upload_payload_rate = 5;
        assert_eq!(state_str(&s), "uploading");
        s.error = "disk full".into();
        assert_eq!(state_str(&s), "error");
        assert!(matches_filter("errored", &s));
        assert!(!matches_filter("seeding", &s));
    }

    #[test]
    fn filters() {
        let p = st(TorrentFlags::PAUSED, TorrentState::Downloading, false);
        assert!(
            matches_filter("paused", &p)
                && matches_filter("stopped", &p)
                && matches_filter("downloading", &p)
        );
        assert!(!matches_filter("running", &p) && !matches_filter("completed", &p));
        let s = st(TorrentFlags::AUTO_MANAGED, TorrentState::Seeding, true);
        assert!(
            matches_filter("seeding", &s) && matches_filter("completed", &s) && matches_filter("stalled", &s)
        );
        assert!(matches_filter("inactive", &s) && !matches_filter("active", &s));
        assert!(matches_filter("whatever", &s));
    }

    #[test]
    fn objects_have_every_field() {
        let mut s = st(TorrentFlags::AUTO_MANAGED, TorrentState::Downloading, false);
        s.info_hashes = InfoHashes::v1(Sha1Hash([0xab; 20]));
        s.name = "a b".into();
        s.total_wanted = 100;
        s.total_wanted_done = 25;
        s.all_time_upload = 10;
        let o = object(&s, &Extra { category: "tv", ..Extra::default() });
        assert_eq!(o.entries().len(), FIELDS.len());
        assert_eq!(o.get("hash").and_then(Json::as_str), Some("ab".repeat(20).as_str()));
        assert_eq!(o.get("progress").and_then(Json::as_f64), Some(0.25));
        assert_eq!(o.get("amount_left").and_then(Json::as_i64), Some(75));
        assert_eq!(o.get("eta").and_then(Json::as_i64), Some(MAX_ETA));
        assert_eq!(o.get("dl_limit").and_then(Json::as_i64), Some(-1));
        assert_eq!(o.get("category").and_then(Json::as_str), Some("tv"));
        assert_eq!(
            o.get("magnet_uri").and_then(Json::as_str),
            Some(format!("magnet:?xt=urn:btih:{}&dn=a%20b", "ab".repeat(20)).as_str())
        );
        assert_eq!(o.get("ratio").and_then(Json::as_f64), Some(0.4));
    }
}
