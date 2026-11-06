//! The alert loop (GTK `on_alerts` and `refresh`'s model part).

use rookparcel_core::{Alert, QueueMove, TorrentFlags};

use super::{Core, Events};
use crate::scheduler::Mode;
use crate::tr;

impl Core {
    /// Drains the engine's alerts (after [`super::Wakeup::Alerts`]) into the list, the detail
    /// tabs and the Logger.
    pub fn on_alerts(&mut self) -> Events {
        self.drain();
        self.take_events()
    }

    /// Pops and handles what the engine queued (and what a dialog's wait saved).
    pub(super) fn drain(&mut self) {
        let alerts = crate::properties::take_alerts(&self.handle, &mut self.alerts);
        self.handle_alerts(alerts);
    }

    pub(super) fn handle_alerts(&mut self, mut alerts: Vec<Alert>) {
        for alert in alerts.drain(..) {
            self.handle_alert(alert);
        }
        // the buffer back, for the next drain (unless a wait saved alerts meanwhile)
        if self.alerts.is_empty() {
            self.alerts = alerts;
        }
    }

    fn handle_alert(&mut self, alert: Alert) {
        // the adapters and the network status (D54)
        crate::network::on_alert(&alert);
        let list = &self.list;
        let label_of = |id| Some(list.extra(id).label.to_owned());
        if let Some((id, dir)) =
            self.move_completed.as_mut().and_then(|m| m.on_alert(&alert, &self.handle, label_of))
        {
            let name = self.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
            self.log(tr!(LogMovingCompleted, name, dir));
            self.handle.post_torrent_updates();
        }
        for line in self.torrent_files.on_alert(&alert, &self.torrent_folders, &self.handle) {
            self.log(line);
        }
        if self.awaiting.on_alert(&alert, &self.handle, &mut self.prefs) {
            self.ev.magnet_ready = true;
        }
        self.ev.status |= self.listen.on_alert(&alert);
        let shown_id = self.detail.id;
        let shown = |id| shown_id == Some(id);
        match alert {
            Alert::StateUpdate(v) => {
                for st in &v {
                    if self.known.insert(st.id) {
                        // restored or added during Seeding only hours
                        if self.scheduler.mode() == Mode::SeedOnly
                            && !st.flags.contains(TorrentFlags::UPLOAD_MODE)
                        {
                            self.handle.torrent(st.id).set_flags(TorrentFlags::UPLOAD_MODE);
                        }
                        // first sight: what only the frontend knows
                        let key = st.info_hashes.key_hex();
                        if let Some(l) = self.prefs.get(&format!("torrent.{key}.label")) {
                            let l = l.to_owned();
                            self.list.set_label(st.id, &l);
                        }
                        if self.prefs.get_bool(&format!("torrent.{key}.paused"), false) {
                            self.list.set_user_paused(st.id, true);
                        }
                    }
                    if !st.is_finished {
                        self.downloading.insert(st.id);
                    }
                    self.seeding_goal(st);
                }
                self.list.update(v);
                self.ev.rows = true;
                self.ev.detail |= self.detail.id.is_some();
            }
            Alert::SessionStats(s) => {
                self.stats = s;
                self.detail.push_speed(s.download_rate, s.upload_rate);
                self.ev.status = true;
                self.ev.detail = true;
            }
            Alert::TorrentAdded { id, info_hashes, ref name } => {
                self.log(tr!(LogAdded, name));
                if self.queue_top.remove(&info_hashes.key_hex()) {
                    self.handle.torrent(id).queue_position(QueueMove::Top);
                }
                self.handle.post_torrent_updates();
            }
            Alert::AddTorrentFailed { ref name, ref error, .. } => {
                self.log(alert.message());
                self.ev.errors.push((tr!(CannotAdd, name), error.to_string()));
            }
            Alert::TorrentRemoved { id, info_hashes } => {
                self.list.remove(id);
                self.known.remove(&id);
                self.downloading.remove(&id);
                let key = info_hashes.key_hex();
                self.prefs.remove(&format!("torrent.{key}.label"));
                self.prefs.remove(&format!("torrent.{key}.paused"));
                // the torrent shown went away
                if shown(id) {
                    self.detail.select(None);
                    self.files_requested = None;
                    self.ev.detail = true;
                }
            }
            Alert::RemoteDisconnected { ref message } => {
                self.log(alert.message());
                self.remote_lost = true;
                self.ev.remote_lost = true;
                self.ev.title = true;
                let label = self.remote.as_ref().map_or("", |r| r.label.as_str());
                self.ev.errors.push((tr!(ErrRemoteLost, label, message), String::new()));
            }
            Alert::TorrentFinished { id } => {
                let name = self.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
                self.log(tr!(LogFinished, name));
                // a download that completed, not a restored or checked seed
                if self.downloading.remove(&id) && self.prefs.get_bool("ui.balloons", true) {
                    self.ev.completed.push(name);
                }
            }
            Alert::FileRenamed { id, .. } | Alert::FileRenameFailed { id, .. } => {
                let name = self.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
                self.log(format!("{name}: {}", alert.message()));
                // a refused Relocate (the target exists: the engine never overwrites a file)
                if let Alert::FileRenameFailed { ref error, .. } = alert {
                    self.ev.errors.push((tr!(CannotRelocate, name), error.to_string()));
                }
                if shown(id) {
                    self.handle.torrent(id).post_file_list();
                }
            }
            Alert::PeerInfo { id, peers } if shown(id) => {
                self.detail.set_peers(peers);
                self.ev.detail = true;
            }
            Alert::TrackerList { id, trackers } if shown(id) => {
                self.detail.trackers = trackers;
                self.ev.detail = true;
            }
            Alert::FileList { id, files } if shown(id) => {
                self.detail.set_files(files);
                self.ev.detail = true;
            }
            Alert::FileProgress { id, done } if shown(id) => {
                self.detail.file_done = done;
                self.ev.detail = true;
            }
            Alert::PieceInfo { id, have, availability, downloading } if shown(id) => {
                self.detail.have = Some(have);
                self.detail.availability = availability;
                self.detail.partial = downloading;
                self.ev.detail = true;
            }
            // standby and hibernation wait for the state on disk
            Alert::ResumeDataSaved => {
                if let Some(p) = self.suspend_pending.take() {
                    self.ev.power = Some(p);
                }
            }
            Alert::PeerInfo { .. }
            | Alert::TrackerList { .. }
            | Alert::FileList { .. }
            | Alert::FileProgress { .. }
            | Alert::PieceInfo { .. }
            | Alert::TorrentFile { .. } => {}
            // the mappers' step-by-step chatter stays out of the Logger, as in the Win32 one
            Alert::PortmapLog { .. } => {}
            // kept by `network::on_alert` for Preferences > Connection
            Alert::NetworkAdapters(_) => {}
            other => {
                let name = other.torrent().and_then(|id| self.list.get(id)).map(|st| st.name.clone());
                match name {
                    Some(n) => self.log(format!("{n}: {}", other.message())),
                    None => self.log(other.message()),
                }
            }
        }
    }
}
