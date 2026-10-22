//! µTorrent 2.2.1 status text and category filters, derived from engine status.
//!
//! libtorrent has no notion of "paused" versus "stopped": both are `PAUSED` without
//! `AUTO_MANAGED`. µTorrent distinguishes them, so frontends track the user's intent
//! (`user_paused`) and pass it in. It will be persisted with the torrent in `core::store` (Phase 2).

use patorrent_core::{TorrentFlags, TorrentState, TorrentStatus};

use crate::icons::StateIcon;
use crate::{fmt, tr};

/// The text shown in the Status column.
pub fn status_text(st: &TorrentStatus, user_paused: bool) -> String {
    if !st.error.is_empty() {
        return tr!(StatusError, error = st.error);
    }
    if st.moving_storage {
        return tr!(StatusMoving).to_owned();
    }
    let paused = st.flags.contains(TorrentFlags::PAUSED);
    let auto = st.flags.contains(TorrentFlags::AUTO_MANAGED);
    match st.state {
        TorrentState::CheckingFiles | TorrentState::CheckingResumeData => {
            return if paused && auto {
                tr!(StatusQueuedForChecking).to_owned()
            } else {
                tr!(StatusChecking, percent = fmt::percent(st.progress_ppm))
            };
        }
        _ => {}
    }
    if paused {
        return match (auto, user_paused, st.is_finished) {
            (true, _, true) => tr!(StatusQueuedSeed),
            (true, _, false) => tr!(StatusQueued),
            (false, true, _) => tr!(StatusPaused),
            (false, false, true) => tr!(StatusFinished),
            (false, false, false) => tr!(StatusStopped),
        }
        .to_owned();
    }
    let what = match st.state {
        TorrentState::DownloadingMetadata => tr!(StatusDownloadingMetadata),
        // µTorrent marks initial seeding (BEP 16 super seeding) with [S], like [F] for forced.
        _ if st.super_seeding => tr!(StatusSuperSeeding),
        _ if st.is_finished => tr!(StatusSeeding),
        _ => tr!(StatusDownloading),
    };
    if auto { what.to_owned() } else { tr!(StatusForced, status = what) }
}

/// A short, stable class for the row's state, with the same branches as [`status_text`]: for
/// frontends that colour or style rows (the Web UI's CSS classes) rather than parse the text.
/// One of `error`, `moving`, `checking`, `queued`, `paused`, `finished`, `stopped`, `metadata`,
/// `seeding`, `downloading`.
pub fn status_class(st: &TorrentStatus, user_paused: bool) -> &'static str {
    if !st.error.is_empty() {
        return "error";
    }
    if st.moving_storage {
        return "moving";
    }
    let paused = st.flags.contains(TorrentFlags::PAUSED);
    let auto = st.flags.contains(TorrentFlags::AUTO_MANAGED);
    if matches!(st.state, TorrentState::CheckingFiles | TorrentState::CheckingResumeData) {
        return if paused && auto { "queued" } else { "checking" };
    }
    if paused {
        return match (auto, user_paused, st.is_finished) {
            (true, _, _) => "queued",
            (false, true, _) => "paused",
            (false, false, true) => "finished",
            (false, false, false) => "stopped",
        };
    }
    match st.state {
        TorrentState::DownloadingMetadata => "metadata",
        _ if st.is_finished => "seeding",
        _ => "downloading",
    }
}

/// The row's PatinaTorrent state icon, with the same branches as [`status_text`]
/// (`assets/patina-icons/states/manifest.json` gives each icon's condition). Unlike the text,
/// the icon keeps checking files and checking resume data apart, and pausing a finished
/// torrent apart from pausing an unfinished one. Initial seeding shows as seeding.
pub fn status_icon(st: &TorrentStatus, user_paused: bool) -> StateIcon {
    if !st.error.is_empty() {
        return StateIcon::Error;
    }
    if st.moving_storage {
        return StateIcon::Moving;
    }
    let paused = st.flags.contains(TorrentFlags::PAUSED);
    let auto = st.flags.contains(TorrentFlags::AUTO_MANAGED);
    match (st.state, paused && auto) {
        (TorrentState::CheckingFiles, true) => return StateIcon::QueuedChecking,
        (TorrentState::CheckingFiles, false) => return StateIcon::CheckingFiles,
        (TorrentState::CheckingResumeData, true) => return StateIcon::QueuedResumeChecking,
        (TorrentState::CheckingResumeData, false) => return StateIcon::CheckingResumeData,
        _ => {}
    }
    if paused {
        return match (auto, user_paused, st.is_finished) {
            (true, _, true) => StateIcon::QueuedSeed,
            (true, _, false) => StateIcon::QueuedDownload,
            (false, true, true) => StateIcon::PausedComplete,
            (false, true, false) => StateIcon::PausedIncomplete,
            (false, false, true) => StateIcon::StoppedComplete,
            (false, false, false) => StateIcon::StoppedIncomplete,
        };
    }
    match (st.state, auto) {
        (TorrentState::DownloadingMetadata, true) => StateIcon::DownloadingMetadata,
        (TorrentState::DownloadingMetadata, false) => StateIcon::ForceDownloadingMetadata,
        _ if st.is_finished => {
            if auto {
                StateIcon::Seeding
            } else {
                StateIcon::ForceSeeding
            }
        }
        (_, true) => StateIcon::Downloading,
        (_, false) => StateIcon::ForceDownloading,
    }
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
            Category::All => tr!(CatTorrents),
            Category::Downloading => tr!(CatDownloading),
            Category::Completed => tr!(CatCompleted),
            Category::Active => tr!(CatActive),
            Category::Inactive => tr!(CatInactive),
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
        let mut superseed = st(am, Seeding, true);
        superseed.super_seeding = true;
        assert_eq!(status_text(&superseed, false), "[S] Seeding");
        superseed.flags = e;
        assert_eq!(status_text(&superseed, false), "[F] [S] Seeding");

        let mut checking = st(e, CheckingFiles, false);
        checking.progress_ppm = 453_000;
        assert_eq!(status_text(&checking, false), "Checking 45.3 %");
        assert_eq!(status_text(&st(am | p, CheckingFiles, false), false), "Queued for checking");

        let mut err = st(am, Downloading, false);
        err.error = "disk full".into();
        assert_eq!(status_text(&err, false), "Error: disk full");
    }

    #[test]
    fn status_classes_follow_the_text() {
        let am = TorrentFlags::AUTO_MANAGED;
        let p = TorrentFlags::PAUSED;
        let e = TorrentFlags::empty();
        use TorrentState::*;
        assert_eq!(status_class(&st(am, Downloading, false), false), "downloading");
        assert_eq!(status_class(&st(e, Seeding, true), false), "seeding");
        assert_eq!(status_class(&st(am | p, Seeding, true), false), "queued");
        assert_eq!(status_class(&st(p, Downloading, false), true), "paused");
        assert_eq!(status_class(&st(p, Downloading, false), false), "stopped");
        assert_eq!(status_class(&st(p, Seeding, true), false), "finished");
        assert_eq!(status_class(&st(am, DownloadingMetadata, false), false), "metadata");
        assert_eq!(status_class(&st(e, CheckingFiles, false), false), "checking");
        assert_eq!(status_class(&st(am | p, CheckingFiles, false), false), "queued");
        let mut err = st(am, Downloading, false);
        err.error = "disk full".into();
        assert_eq!(status_class(&err, false), "error");
        err.error.clear();
        err.moving_storage = true;
        assert_eq!(status_class(&err, false), "moving");
    }

    #[test]
    fn status_icons_follow_the_manifest() {
        let am = TorrentFlags::AUTO_MANAGED;
        let p = TorrentFlags::PAUSED;
        let e = TorrentFlags::empty();
        use StateIcon as I;
        use TorrentState::*;
        let cases = [
            (st(am, Downloading, false), false, I::Downloading),
            (st(e, Downloading, false), false, I::ForceDownloading),
            (st(am, Seeding, true), false, I::Seeding),
            (st(e, Seeding, true), false, I::ForceSeeding),
            // the core's Finished (wanted pieces done) is shown as seeding, as the text does
            (st(am, Finished, true), false, I::Seeding),
            (st(am, DownloadingMetadata, false), false, I::DownloadingMetadata),
            (st(e, DownloadingMetadata, false), false, I::ForceDownloadingMetadata),
            (st(e, CheckingFiles, false), false, I::CheckingFiles),
            (st(am, CheckingResumeData, false), false, I::CheckingResumeData),
            (st(am | p, CheckingFiles, false), false, I::QueuedChecking),
            (st(am | p, CheckingResumeData, false), false, I::QueuedResumeChecking),
            (st(am | p, Downloading, false), false, I::QueuedDownload),
            (st(am | p, Seeding, true), false, I::QueuedSeed),
            (st(p, Downloading, false), true, I::PausedIncomplete),
            (st(p, Seeding, true), true, I::PausedComplete),
            (st(p, Downloading, false), false, I::StoppedIncomplete),
            (st(p, Seeding, true), false, I::StoppedComplete),
        ];
        for (s, user_paused, icon) in cases {
            assert_eq!(status_icon(&s, user_paused), icon, "{:?} {:?}", s.state, s.flags);
        }
        let mut s = st(am, Downloading, false);
        s.moving_storage = true;
        assert_eq!(status_icon(&s, false), I::Moving);
        s.error = "disk full".into();
        assert_eq!(status_icon(&s, false), I::Error);
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
