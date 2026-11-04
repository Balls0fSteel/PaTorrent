//! µTorrent 2.2.1 status text and category filters, derived from engine status.
//!
//! libtorrent has no notion of "paused" versus "stopped": both are `PAUSED` without
//! `AUTO_MANAGED`. µTorrent distinguishes them, so frontends track the user's intent
//! (`user_paused`) and pass it in. It will be persisted with the torrent in `core::store` (Phase 2).

use patorrent_core::{Alert, Protocol, SessionStats, TorrentFlags, TorrentState, TorrentStatus};

use crate::icons::StateIcon;
use crate::{fmt, tr};

/// A checking state that is (or is queued to be) checking. A torrent added stopped stays in
/// `CheckingResumeData` until it first starts; stopped or paused, it shows as such.
fn checking(st: &TorrentStatus) -> bool {
    matches!(st.state, TorrentState::CheckingFiles | TorrentState::CheckingResumeData)
        && (!st.flags.contains(TorrentFlags::PAUSED) || st.flags.contains(TorrentFlags::AUTO_MANAGED))
}

/// The status bar's download and upload parts, with Keep ping low's cap while it limits a
/// direction (D49).
pub fn bar_rates(s: &SessionStats) -> [String; 2] {
    let down = (fmt::speed_always(s.download_rate), fmt::size(s.recv_payload_bytes));
    let up = (fmt::speed_always(s.upload_rate), fmt::size(s.sent_payload_bytes));
    [
        match s.auto_download_limit {
            0 => tr!(BarDown, rate = down.0, total = down.1),
            cap => tr!(BarDownAuto, rate = down.0, cap = fmt::speed_always(cap), total = down.1),
        },
        match s.auto_upload_limit {
            0 => tr!(BarUp, rate = up.0, total = up.1),
            cap => tr!(BarUpAuto, rate = up.0, cap = fmt::speed_always(cap), total = up.1),
        },
    ]
}

/// The configured listen port another program holds (another profile, D42), for the status
/// bar's network part. The engine then listens on a later port (libtorrent's
/// `max_retry_port_bind`), where nobody forwards anything: a TCP `ListenSucceeded` on another
/// port than the configured one marks its IP, as does a TCP `ListenFailed` for "address in
/// use"; a `ListenSucceeded` on the configured port clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenProblem {
    /// The port from the settings; `None` or 0 when any port will do.
    configured: Option<u16>,
    in_use: Vec<std::net::SocketAddr>,
}

impl ListenProblem {
    pub fn new(configured: Option<u16>) -> ListenProblem {
        ListenProblem { configured, in_use: Vec::new() }
    }

    /// The settings changed: the sockets are opened again and report anew.
    pub fn set_configured(&mut self, port: Option<u16>) {
        if self.configured != port {
            self.configured = port;
            self.in_use.clear();
        }
    }

    /// Follows `a`; true when the text changed.
    pub fn on_alert(&mut self, a: &Alert) -> bool {
        let before = self.in_use.clone();
        let wanted = self.configured.filter(|&p| p != 0);
        match a {
            Alert::ListenFailed {
                address,
                protocol: Protocol::Tcp,
                error: patorrent_core::Error::Io { kind: std::io::ErrorKind::AddrInUse, .. },
            } => {
                if let Ok(addr) = address.parse::<std::net::SocketAddr>() {
                    self.in_use.retain(|a| a.ip() != addr.ip());
                    self.in_use.push(addr);
                }
            }
            Alert::ListenSucceeded { address, protocol: Protocol::Tcp } => {
                self.in_use.retain(|a| a.ip() != address.ip());
                if let Some(p) = wanted.filter(|&p| p != address.port()) {
                    self.in_use.push(std::net::SocketAddr::new(address.ip(), p));
                }
            }
            _ => {}
        }
        self.in_use != before
    }

    /// The port that could not be opened, if any.
    pub fn port(&self) -> Option<u16> {
        self.in_use.first().map(std::net::SocketAddr::port)
    }

    /// "Port 6881 in use", in place of the network part's usual text.
    pub fn text(&self) -> Option<String> {
        self.port().map(|port| tr!(NetPortInUse, port))
    }
}

/// The text shown in the Status column.
pub fn status_text(st: &TorrentStatus, user_paused: bool) -> String {
    if !st.error.is_empty() {
        return tr!(StatusError, error = st.error);
    }
    if st.moving_storage {
        return tr!(StatusMoving).to_owned();
    }
    if st.waiting_for_network {
        return tr!(StatusWaitingNetwork).to_owned();
    }
    let paused = st.flags.contains(TorrentFlags::PAUSED);
    let auto = st.flags.contains(TorrentFlags::AUTO_MANAGED);
    if checking(st) {
        return if paused {
            tr!(StatusQueuedForChecking).to_owned()
        } else {
            tr!(StatusChecking, percent = fmt::percent(st.progress_ppm))
        };
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
/// One of `error`, `moving`, `network` (waiting for the network), `checking`, `queued`,
/// `paused`, `finished`, `stopped`, `metadata`, `seeding`, `downloading`.
pub fn status_class(st: &TorrentStatus, user_paused: bool) -> &'static str {
    if !st.error.is_empty() {
        return "error";
    }
    if st.moving_storage {
        return "moving";
    }
    if st.waiting_for_network {
        return "network";
    }
    let paused = st.flags.contains(TorrentFlags::PAUSED);
    let auto = st.flags.contains(TorrentFlags::AUTO_MANAGED);
    if checking(st) {
        return if paused { "queued" } else { "checking" };
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

/// The row's state icon, with the same branches as [`status_text`]
/// (`assets/patina-icons/states/manifest.json` gives each state's condition; the artwork is
/// the bold set's, `assets/ui-icon-explorations`). Unlike the text,
/// the icon keeps checking files and checking resume data apart, and pausing a finished
/// torrent apart from pausing an unfinished one. Initial seeding shows as seeding.
pub fn status_icon(st: &TorrentStatus, user_paused: bool) -> StateIcon {
    if !st.error.is_empty() {
        return StateIcon::Error;
    }
    if st.moving_storage {
        return StateIcon::Moving;
    }
    // held by the session (D54): shown as queued, which it resembles
    if st.waiting_for_network {
        return if st.is_finished { StateIcon::QueuedSeed } else { StateIcon::QueuedDownload };
    }
    let paused = st.flags.contains(TorrentFlags::PAUSED);
    let auto = st.flags.contains(TorrentFlags::AUTO_MANAGED);
    if checking(st) {
        return match (st.state, paused) {
            (TorrentState::CheckingFiles, true) => StateIcon::QueuedChecking,
            (TorrentState::CheckingFiles, false) => StateIcon::CheckingFiles,
            (_, true) => StateIcon::QueuedResumeChecking,
            (_, false) => StateIcon::CheckingResumeData,
        };
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
    fn a_port_in_use() {
        let in_use = |address: &str| Alert::ListenFailed {
            address: address.into(),
            protocol: Protocol::Tcp,
            error: std::io::Error::from(std::io::ErrorKind::AddrInUse).into(),
        };
        let mut p = ListenProblem::new(Some(6881));
        assert_eq!(p.text(), None);
        // UDP and other failures are the Logger's
        let udp = Alert::ListenFailed {
            address: "0.0.0.0:6881".into(),
            protocol: Protocol::Udp,
            error: std::io::Error::from(std::io::ErrorKind::AddrInUse).into(),
        };
        let other = Alert::ListenFailed {
            address: "eth9:6881".into(),
            protocol: Protocol::Tcp,
            error: std::io::Error::from(std::io::ErrorKind::InvalidInput).into(),
        };
        assert!(!p.on_alert(&udp) && !p.on_alert(&other));
        assert!(p.on_alert(&in_use("0.0.0.0:6881")));
        assert_eq!(p.port(), Some(6881));
        assert!(p.text().unwrap().contains("6881"));
        // IPv6 bound: the IPv4 problem stays
        let ok = |a: &str| Alert::ListenSucceeded { address: a.parse().unwrap(), protocol: Protocol::Tcp };
        assert!(!p.on_alert(&ok("[::]:6881")));
        // the configured port bound later clears it
        assert!(p.on_alert(&ok("0.0.0.0:6881")));
        assert_eq!(p.text(), None);
        // the engine moved on to the next port: the configured one is in use
        assert!(p.on_alert(&ok("0.0.0.0:6882")));
        assert_eq!(p.port(), Some(6881));
        // Preferences picked another port, which bound
        p.set_configured(Some(7000));
        assert_eq!(p.text(), None);
        assert!(!p.on_alert(&ok("0.0.0.0:7000")));
        // any port will do: nothing to say
        let mut any = ListenProblem::new(Some(0));
        assert!(!any.on_alert(&ok("0.0.0.0:51413")));
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
        // added stopped: the engine has not begun its check
        assert_eq!(status_text(&st(p, CheckingResumeData, false), false), "Stopped");
        assert_eq!(status_text(&st(p, CheckingResumeData, false), true), "Paused");

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
        assert_eq!(status_class(&st(p, CheckingResumeData, false), false), "stopped");
        assert_eq!(status_class(&st(p, CheckingResumeData, false), true), "paused");
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
            (st(p, CheckingResumeData, false), false, I::StoppedIncomplete),
            (st(p, CheckingFiles, false), true, I::PausedIncomplete),
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
