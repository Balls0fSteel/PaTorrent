//! µTorrent 2.2.1 status text and category filters, derived from engine status.
//!
//! libtorrent has no notion of "paused" versus "stopped": both are `PAUSED` without
//! `AUTO_MANAGED`. µTorrent distinguishes them, so frontends track the user's intent
//! (`user_paused`) and pass it in. It will be persisted with the torrent in `core::store` (Phase 2).

use patorrent_core::{TorrentFlags, TorrentState, TorrentStatus};

use crate::fmt;

/// The text shown in the Status column.
pub fn status_text(st: &TorrentStatus, user_paused: bool) -> String {
    if !st.error.is_empty() {
        return format!("Error: {}", st.error);
    }
    if st.moving_storage {
        return "Moving".to_owned();
    }
    let paused = st.flags.contains(TorrentFlags::PAUSED);
    let auto = st.flags.contains(TorrentFlags::AUTO_MANAGED);
    match st.state {
        TorrentState::CheckingFiles | TorrentState::CheckingResumeData => {
            return if paused && auto {
                "Queued for checking".to_owned()
            } else {
                format!("Checking {}", fmt::percent(st.progress_ppm))
            };
        }
        _ => {}
    }
    if paused {
        return match (auto, user_paused, st.is_finished) {
            (true, _, true) => "Queued Seed",
            (true, _, false) => "Queued",
            (false, true, _) => "Paused",
            (false, false, true) => "Finished",
            (false, false, false) => "Stopped",
        }
        .to_owned();
    }
    let what = match st.state {
        TorrentState::DownloadingMetadata => "Downloading metadata",
        _ if st.is_finished => "Seeding",
        _ => "Downloading",
    };
    if auto { what.to_owned() } else { format!("[F] {what}") }
}

/// The fixed entries of µTorrent's category list. Labels and feeds join in Phase 5.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Category {
    All,
    Downloading,
    Completed,
    /// Moving data right now.
    Active,
    Inactive,
}

impl Category {
    pub const ALL: [Category; 5] =
        [Category::All, Category::Downloading, Category::Completed, Category::Active, Category::Inactive];

    pub fn label(self) -> &'static str {
        match self {
            Category::All => "Torrents",
            Category::Downloading => "Downloading",
            Category::Completed => "Completed",
            Category::Active => "Active",
            Category::Inactive => "Inactive",
        }
    }

    pub fn contains(self, st: &TorrentStatus) -> bool {
        let active = st.download_payload_rate > 0 || st.upload_payload_rate > 0;
        match self {
            Category::All => true,
            Category::Downloading => !st.is_finished,
            Category::Completed => st.is_finished,
            Category::Active => active,
            Category::Inactive => !active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(flags: TorrentFlags, state: TorrentState, finished: bool) -> TorrentStatus {
        TorrentStatus { flags, state, is_finished: finished, ..TorrentStatus::default() }
    }

    #[test]
    fn status_strings() {
        let am = TorrentFlags::AUTO_MANAGED;
        let p = TorrentFlags::PAUSED;
        let e = TorrentFlags::empty();
        use TorrentState::*;
        assert_eq!(status_text(&st(am, Downloading, false), false), "Downloading");
        assert_eq!(status_text(&st(e, Downloading, false), false), "[F] Downloading");
        assert_eq!(status_text(&st(am, Seeding, true), false), "Seeding");
        assert_eq!(status_text(&st(e, Seeding, true), false), "[F] Seeding");
        assert_eq!(status_text(&st(am | p, Downloading, false), false), "Queued");
        assert_eq!(status_text(&st(am | p, Seeding, true), false), "Queued Seed");
        assert_eq!(status_text(&st(p, Downloading, false), true), "Paused");
        assert_eq!(status_text(&st(p, Downloading, false), false), "Stopped");
        assert_eq!(status_text(&st(p, Seeding, true), false), "Finished");
        assert_eq!(status_text(&st(am, DownloadingMetadata, false), false), "Downloading metadata");

        let mut checking = st(e, CheckingFiles, false);
        checking.progress_ppm = 453_000;
        assert_eq!(status_text(&checking, false), "Checking 45.3 %");
        assert_eq!(status_text(&st(am | p, CheckingFiles, false), false), "Queued for checking");

        let mut err = st(am, Downloading, false);
        err.error = "disk full".into();
        assert_eq!(status_text(&err, false), "Error: disk full");
    }

    #[test]
    fn categories() {
        let mut s = st(TorrentFlags::AUTO_MANAGED, TorrentState::Downloading, false);
        assert!(Category::Downloading.contains(&s) && !Category::Completed.contains(&s));
        assert!(Category::Inactive.contains(&s));
        s.upload_payload_rate = 1;
        assert!(Category::Active.contains(&s) && !Category::Inactive.contains(&s));
        assert!(Category::ALL.iter().all(|c| !c.label().is_empty()));
    }
}
