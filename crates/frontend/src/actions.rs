//! µTorrent's torrent actions expressed with libtorrent's flags.
//!
//! | µTorrent    | libtorrent                          | Shows as                         |
//! |-------------|-------------------------------------|----------------------------------|
//! | Start       | set `AUTO_MANAGED`, `resume()`      | Downloading/Seeding or Queued    |
//! | Force Start | unset `AUTO_MANAGED`, `resume()`    | [F] Downloading / [F] Seeding    |
//! | Pause       | unset `AUTO_MANAGED`, `pause()`     | Paused (frontend remembers)      |
//! | Stop        | unset `AUTO_MANAGED`, `pause()`     | Stopped / Finished               |
//!
//! Pause and Stop are the same in the engine. The frontend records which one the user chose
//! (see [`crate::status::status_text`]).

use patorrent_core::{TorrentFlags, TorrentHandle};

pub fn start(t: &TorrentHandle) {
    t.set_flags(TorrentFlags::AUTO_MANAGED);
    t.resume();
}

pub fn force_start(t: &TorrentHandle) {
    t.unset_flags(TorrentFlags::AUTO_MANAGED);
    t.resume();
}

/// Also used for Pause; the caller records the difference.
pub fn stop(t: &TorrentHandle) {
    t.unset_flags(TorrentFlags::AUTO_MANAGED);
    t.pause();
}

#[cfg(test)]
mod tests {
    use super::*;
    use patorrent_core::{AddTorrentParams, Alert, MockConfig, Session, Settings, TorrentSource};
    use std::time::{Duration, Instant};

    #[test]
    fn actions_drive_engine_flags() {
        let s =
            Session::start_mock(Settings { tick_interval: 50, ..Settings::default() }, MockConfig::default());
        let h = s.handle();
        let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=t".to_owned();
        h.add_torrent(AddTorrentParams::new(TorrentSource::Magnet(magnet), "/dl"));

        let mut alerts = Vec::new();
        let mut id = None;
        let mut flags = None;
        let mut step = 0;
        let deadline = Instant::now() + Duration::from_secs(10);
        while step < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            h.post_torrent_updates();
            h.pop_alerts(&mut alerts);
            for a in &alerts {
                match a {
                    Alert::TorrentAdded { id: x, .. } => id = Some(*x),
                    Alert::StateUpdate(v) => {
                        if let Some(st) = v.iter().find(|st| Some(st.id) == id) {
                            flags = Some(st.flags);
                        }
                    }
                    _ => {}
                }
            }
            let (Some(id), Some(f)) = (id, flags) else { continue };
            let t = h.torrent(id);
            let auto = f.contains(TorrentFlags::AUTO_MANAGED);
            let paused = f.contains(TorrentFlags::PAUSED);
            match step {
                0 if auto && !paused => {
                    force_start(&t);
                    step = 1;
                }
                1 if !auto && !paused => {
                    stop(&t);
                    step = 2;
                }
                2 if !auto && paused => step = 3,
                _ => {}
            }
        }
        assert_eq!(step, 3, "start -> force start -> stop did not complete");
    }
}
