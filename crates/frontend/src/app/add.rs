//! Adding torrents (GTK `add_torrent_file`, `add_magnet`, `magnet_dialogs`, `add`,
//! `add_to_session`, `download`, `import_from`'s logic, `create_torrent`'s add).

use std::path::PathBuf;
use std::sync::Arc;

use rookparcel_core::fetch::{FetchOptions, http_get};
use rookparcel_core::metainfo::{TorrentInfo, parse_magnet_uri};
use rookparcel_core::{AddTorrentParams, RemoveFlags, TorrentId, TorrentSource};

use super::{Core, Wakeup};
use crate::add::{AddChoice, AwaitingMetadata, Setup, remember_dir};
use crate::import::{Client, Found};
use crate::rss::Download;
use crate::scheduler::Mode;
use crate::{create, tr};

/// An Add New Torrent dialog to show: for a `.torrent` file, or for a magnet whose metadata
/// arrived. The GUI shows `ti` with `setup` and hands the answer to [`Core::finish_add`].
#[derive(Clone, Debug)]
pub struct AddRequest {
    /// The torrent's metadata (name, files).
    pub ti: Arc<TorrentInfo>,
    /// What the dialog starts with.
    pub setup: Setup,
    /// Its info-hash (`InfoHashes::key_hex`).
    pub key: String,
    source: Source,
}

#[derive(Clone, Debug)]
enum Source {
    /// A `.torrent` file's contents, not in the session yet.
    File(Vec<u8>),
    /// A magnet in the session (upload mode, [`AwaitingMetadata`]) added to `save_path`.
    Magnet { id: TorrentId, save_path: String },
}

impl AddRequest {
    /// The torrent is a magnet already in the session (its files were checked: "Skip hash
    /// check" is greyed, `setup.in_session`).
    pub fn is_magnet(&self) -> bool {
        matches!(self.source, Source::Magnet { .. })
    }

    /// The torrent's name.
    pub fn name(&self) -> &str {
        self.ti.name()
    }
}

/// What came of adding a torrent.
#[derive(Debug)]
pub enum AddStep {
    /// Show the Add New Torrent dialog.
    Dialog(AddRequest),
    /// Handed to the engine (it reports failures as an error in [`super::Events`]).
    Added,
    /// A magnet joined the session to fetch its metadata; the dialog comes with
    /// [`super::Events::magnet_ready`].
    Awaiting,
    /// The torrent is in the list already: show `tr!(InfoAlreadyInList, name)`.
    AlreadyInList(String),
    /// A `.torrent` is being downloaded; what follows comes with [`Wakeup::Fetched`].
    Fetching,
    /// Show this error: (message, detail).
    Error(String, String),
    /// Nothing to do (an empty text).
    Nothing,
}

impl Core {
    /// A `.torrent`'s contents: through the Add New Torrent dialog unless Preferences turned it
    /// off (files the engine cannot read are handed to it anyway, so it reports why).
    pub fn add_file(&mut self, bytes: Vec<u8>) -> AddStep {
        let gui = self.gui_options();
        let ti = match TorrentInfo::from_bytes(&bytes) {
            Ok(ti) if gui.show_add_dialog => ti,
            _ => {
                self.add(TorrentSource::TorrentFile(bytes));
                return AddStep::Added;
            }
        };
        let key = ti.info_hashes().key_hex();
        if self.in_list(&key).is_some() {
            return AddStep::AlreadyInList(ti.name().to_owned());
        }
        let setup = Setup::new(&gui, &self.prefs, self.labels());
        AddStep::Dialog(AddRequest { ti: Arc::new(ti), setup, key, source: Source::File(bytes) })
    }

    /// A magnet link. With the Add New Torrent dialog on, the magnet joins the session in
    /// upload mode to fetch its metadata, and the dialog opens once the file list is known
    /// ([`AwaitingMetadata`], [`Core::next_magnet_dialog`]); otherwise it is added directly.
    pub fn add_magnet(&mut self, uri: String) -> AddStep {
        let show = self.gui_options().show_add_dialog;
        let key = match parse_magnet_uri(&uri) {
            Ok(m) if show => m.info_hashes.key_hex(),
            // the engine reports a broken link
            _ => {
                self.add(TorrentSource::Magnet(uri));
                return AddStep::Added;
            }
        };
        if let Some(name) = self.in_list(&key) {
            return AddStep::AlreadyInList(name);
        }
        let save_path = self.save_path.clone();
        let mut p = AddTorrentParams::new(TorrentSource::Magnet(uri), save_path.clone());
        self.gui_options().apply_to_new(&mut p);
        AwaitingMetadata::prepare(&mut p);
        self.awaiting.insert(&mut self.prefs, &key, &save_path);
        self.save_prefs();
        self.add_to_session(p);
        AddStep::Awaiting
    }

    /// Add Torrent from URL's OK with the text typed: a magnet, the `http(s)://` URL of a
    /// `.torrent` (fetched on a worker thread), or an error for anything else.
    pub fn add_url(&mut self, text: &str) -> AddStep {
        let url = text.trim();
        if url.starts_with("magnet:") || url.starts_with("http://") || url.starts_with("https://") {
            self.add_link(url.to_owned())
        } else if url.is_empty() {
            AddStep::Nothing
        } else {
            AddStep::Error(tr!(ErrNotALink).to_owned(), url.to_owned())
        }
    }

    /// A `magnet:` link or the `http(s)://` URL of a `.torrent` (the command line, the system
    /// opening a link).
    pub fn add_link(&mut self, link: String) -> AddStep {
        if link.starts_with("magnet:") {
            self.add_magnet(link)
        } else {
            self.download(link);
            AddStep::Fetching
        }
    }

    /// What a search or the RSS window downloaded.
    pub fn add_download(&mut self, d: Download) -> AddStep {
        match d {
            Download::Torrent(bytes) => self.add_file(bytes),
            Download::Magnet(uri) => self.add_magnet(uri),
        }
    }

    /// The Add New Torrent dialog's answer (`None`: Cancel). For a `.torrent` OK adds it with
    /// the choices; for a magnet see [`Core::finish_magnet`], whose next dialog is returned.
    pub fn finish_add(&mut self, r: AddRequest, choice: Option<AddChoice>) -> Option<AddRequest> {
        let bytes = match r.source {
            Source::Magnet { .. } => return self.finish_magnet(r, choice),
            Source::File(bytes) => bytes,
        };
        let choice = choice?;
        let key = r.key;
        remember_dir(&mut self.prefs, &choice.save_path);
        if !choice.label.is_empty() {
            // picked up when the torrent first shows up in the list, like a restored label
            self.prefs.set(&format!("torrent.{key}.label"), &choice.label);
        }
        if choice.top_of_queue {
            self.queue_top.insert(key);
        }
        self.save_prefs();
        let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes), self.save_path.clone());
        self.gui_options().apply_to_new(&mut p);
        choice.apply_to(&r.ti, &mut p);
        self.add_to_session(p);
        None
    }

    /// The Add New Torrent dialog for the next magnet whose metadata arrived; `None` while one
    /// is open (they open one at a time) or none is ready.
    pub fn next_magnet_dialog(&mut self) -> Option<AddRequest> {
        if self.awaiting_dialog {
            return None;
        }
        let (key, id, ti, save_path) = self.awaiting.ready().into_iter().find_map(|key| {
            let w = self.awaiting.get(&key)?;
            Some((key.clone(), w.id?, w.ti.clone()?, w.save_path.clone()))
        })?;
        let setup = Setup::for_magnet(&self.gui_options(), &self.prefs, self.labels(), &save_path);
        self.awaiting_dialog = true;
        Some(AddRequest { ti, setup, key, source: Source::Magnet { id, save_path } })
    }

    /// A magnet's dialog answered: OK gives the torrent the choices and starts it (or leaves it
    /// stopped); Cancel removes it with its part file (nothing else was written). Returns the
    /// next magnet's dialog, if another is ready.
    pub fn finish_magnet(&mut self, r: AddRequest, choice: Option<AddChoice>) -> Option<AddRequest> {
        let Source::Magnet { id, save_path } = r.source else {
            self.finish_add(r, choice);
            return self.next_magnet_dialog();
        };
        let key = r.key;
        self.awaiting_dialog = false;
        self.awaiting.finish(&mut self.prefs, &key);
        match choice {
            Some(c) => {
                remember_dir(&mut self.prefs, &c.save_path);
                if !c.label.is_empty() {
                    self.prefs.set(&format!("torrent.{key}.label"), &c.label);
                    self.list.set_label(id, &c.label);
                }
                let seed_only = self.scheduler.mode() == Mode::SeedOnly;
                c.apply_to_torrent(&self.handle.torrent(id), &r.ti, &save_path, seed_only);
                self.handle.post_torrent_updates();
            }
            None => self.handle.remove_torrent(id, RemoveFlags::DELETE_PARTFILE),
        }
        self.save_prefs();
        self.next_magnet_dialog()
    }

    /// File > Import's last step, after the user said yes to [`crate::import::begin`]'s
    /// question: the labels and pause marks go to the preferences (read when the torrents
    /// first show up), the torrents to the engine. Returns the summary to show.
    pub fn import(&mut self, client: Client, found: Found) -> String {
        let keys: std::collections::HashSet<String> =
            self.list.ids().filter_map(|id| self.list.get(id)).map(|s| s.info_hashes.key_hex()).collect();
        let done = crate::import::finish(client, found, &mut self.prefs, |key| keys.contains(key));
        self.save_prefs();
        for line in &done.log {
            self.log(line);
        }
        for p in done.add {
            self.add_to_session(p);
        }
        done.summary
    }

    /// Create New Torrent closed: its fields are remembered; `seed` (the new `.torrent` and the
    /// source's parent folder, with "Start seeding") adds it to seed.
    pub fn created(&mut self, r: create::Remembered, seed: Option<(Vec<u8>, PathBuf)>) {
        r.store(&mut self.prefs);
        self.save_prefs();
        if let Some((bytes, save)) = seed {
            self.add_to_session(create::seed_params(bytes, &save));
        }
    }

    /// Adds without the dialog, with the Preferences' defaults.
    pub(super) fn add(&mut self, source: TorrentSource) {
        let mut p = AddTorrentParams::new(source, self.save_path.clone());
        self.gui_options().apply_to_new(&mut p);
        self.add_to_session(p);
    }

    /// Hands a torrent to the engine (in upload mode during Seeding only hours); a `.torrent`'s
    /// copy is stored once it is added.
    pub(super) fn add_to_session(&mut self, mut p: AddTorrentParams) {
        self.scheduler.apply_to_new(&mut p);
        self.torrent_files.adding(&self.torrent_folders, &p);
        self.handle.add_torrent(p);
    }

    /// Fetches a `.torrent` on a worker thread (through the session's proxy, as the RSS
    /// fetches do); [`Wakeup::Fetched`] brings it back.
    fn download(&mut self, url: String) {
        let proxy = self.proxy();
        let (slot, wake) = (self.fetched.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let result = http_get(&url, &FetchOptions { proxy, ..FetchOptions::default() })
                .map_err(|e| e.to_string())
                .and_then(|r| if r.status == 200 { Ok(r.body) } else { Err(format!("HTTP {}", r.status)) });
            slot.lock().unwrap_or_else(|e| e.into_inner()).push((url, result));
            wake(Wakeup::Fetched);
        });
    }

    /// The finished downloads: their dialogs, messages or errors into the events.
    pub(super) fn fetched_done(&mut self) {
        let done = std::mem::take(&mut *self.fetched.lock().unwrap_or_else(|e| e.into_inner()));
        for (url, result) in done {
            match result {
                Ok(bytes) => match self.add_file(bytes) {
                    AddStep::Dialog(r) => self.ev.adds.push(r),
                    AddStep::AlreadyInList(name) => self.ev.infos.push(tr!(InfoAlreadyInList, name)),
                    _ => {}
                },
                Err(e) => self.ev.errors.push((tr!(CannotDownload, url), e)),
            }
        }
    }
}
