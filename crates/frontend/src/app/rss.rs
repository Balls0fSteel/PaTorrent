//! The RSS Downloader and the feed view (GTK's RSS functions, model part): the timer starts the
//! refreshes that are due, the fetches run on the worker's threads and wake the GUI
//! ([`super::Wakeup::Rss`]), and [`Core::on_wake`] takes their results and adds what the
//! filters chose. Also what the search windows ask of the main window.

use rookparcel_core::AddTorrentParams;

use super::Core;
use crate::rss::view::{FeedForm, ItemRow, item_rows};
use crate::rss::{AddRequest, Download, FeedId, RssDownloader};
use crate::search::{Source as SearchSource, load_sources, store_sources};
use crate::tr;

impl Core {
    /// Once a second: the refreshes that are due, and `rss.dat` when it changed.
    pub(super) fn rss_timer(&mut self) {
        let now = self.unix_now();
        if self.rss.next_due().is_some_and(|t| t <= now)
            && self.rss.due_feeds(now).iter().any(|&id| !self.rss_worker.is_busy(id))
        {
            let proxy = self.proxy();
            self.rss_worker.refresh_due(&self.rss, now, proxy.as_ref());
        }
        if self.rss_dirty {
            self.rss_dirty = false;
            let file = self.rss_file();
            if let Err(e) = self.rss.save(&file) {
                self.log(tr!(LogNotSaved, file = file.display(), error = e));
            }
        }
    }

    /// Starts refreshing feed `feed` now (`None`: every enabled feed): the RSS window's and the
    /// Feeds row menu's Update.
    pub fn update_feed(&mut self, feed: Option<FeedId>) {
        let proxy = self.proxy();
        let ids: Vec<FeedId> = match feed {
            Some(id) => vec![id],
            None => self.rss.feeds.iter().filter(|f| f.enabled).map(|f| f.id).collect(),
        };
        for id in ids {
            self.rss_worker.refresh(&self.rss, id, proxy.as_ref());
        }
    }

    /// A fetch finished: the results into the downloader, and their torrents into the session.
    pub(super) fn rss_collect(&mut self) {
        let proxy = self.proxy();
        let now = self.unix_now();
        let c = self.rss_worker.collect(&mut self.rss, now, proxy.as_ref());
        for line in c.log {
            self.log(line);
        }
        for (req, d) in c.adds {
            self.rss_add(req, d);
        }
        self.rss_dirty |= c.changed;
        if c.changed {
            self.rss_changed();
        }
    }

    /// What follows the downloader: the RSS window, the Feeds rows and the feed items.
    fn rss_changed(&mut self) {
        self.ev.feeds = true;
        self.ev.sidebar = true;
        if let Some(view) = self.feed_view {
            self.feed_rows = item_rows(&self.rss, view);
        }
    }

    /// The sidebar's Feeds rows: (id, name).
    pub fn feeds(&self) -> Vec<(FeedId, String)> {
        self.rss.feeds.iter().map(|f| (f.id, f.name().to_owned())).collect()
    }

    /// Shows feed items in the list (`Some(None)`: every feed's; `Some(Some(id))`: one feed's)
    /// or, with `None`, the torrents.
    pub fn show_feed(&mut self, view: Option<Option<FeedId>>) {
        self.feed_view = view;
        self.feed_rows = view.map(|f| item_rows(&self.rss, f)).unwrap_or_default();
        self.ev.feeds = true;
    }

    /// The feed the list shows the items of, if it shows any.
    pub fn feed_view(&self) -> Option<Option<FeedId>> {
        self.feed_view
    }

    /// The feed items the list shows ([`crate::rss::view::ITEM_COLUMNS`]).
    pub fn feed_rows(&self) -> &[ItemRow] {
        &self.feed_rows
    }

    /// Downloads the feed items in `rows` (a double click, or the menu's Download).
    pub fn download_feed_items(&mut self, rows: &[usize]) {
        let now = self.unix_now();
        let picked: Vec<(FeedId, String)> =
            rows.iter().filter_map(|&r| self.feed_rows.get(r)).map(|r| (r.feed, r.id.clone())).collect();
        let reqs: Vec<AddRequest> =
            picked.into_iter().filter_map(|(feed, id)| self.rss.request_item(feed, &id, now)).collect();
        self.rss_download(reqs);
        if let Some(view) = self.feed_view {
            self.feed_rows = item_rows(&self.rss, view);
            self.ev.feeds = true;
        }
    }

    /// The downloader, for the RSS window.
    pub fn rss(&self) -> &RssDownloader {
        &self.rss
    }

    /// Changes the downloader (the RSS window's edits); it is saved at the next second.
    pub fn edit_rss(&mut self, f: impl FnOnce(&mut RssDownloader)) {
        f(&mut self.rss);
        self.rss_dirty = true;
    }

    /// Feed `id` is being fetched (the RSS window's status column).
    pub fn feed_updating(&self, id: FeedId) -> bool {
        self.rss_worker.is_busy(id)
    }

    /// Whether a Feeds row menu offers Disable (`true`) or Enable for `feed` (`None`: the
    /// Feeds row itself, whose Edit, Disable and Delete are greyed).
    pub fn feed_enabled(&self, feed: Option<FeedId>) -> bool {
        feed.and_then(|id| self.rss.feed(id)).is_none_or(|f| f.enabled)
    }

    /// Feed `id`'s name (Delete Feed's question).
    pub fn feed_name(&self, id: FeedId) -> Option<String> {
        self.rss.feed(id).map(|f| f.name().to_owned())
    }

    /// Add RSS Feed's OK: the feed is added and fetched.
    pub fn add_feed(&mut self, form: &FeedForm) -> FeedId {
        let id = form.add_to(&mut self.rss);
        self.rss_dirty = true;
        self.update_feed(Some(id));
        self.rss_changed();
        id
    }

    /// Edit Feed's OK: fetched again if its address changed.
    pub fn edit_feed(&mut self, id: FeedId, form: &FeedForm) {
        if form.store(&mut self.rss, id) {
            self.update_feed(Some(id));
        }
        self.rss_dirty = true;
        self.rss_changed();
    }

    /// Disable Feed / Enable Feed.
    pub fn toggle_feed(&mut self, id: FeedId) {
        if let Some(f) = self.rss.feed_mut(id) {
            f.enabled = !f.enabled;
            self.rss_dirty = true;
        }
    }

    /// Delete Feed, after its question (`AskDeleteFeed`).
    pub fn remove_feed(&mut self, id: FeedId) {
        self.rss.remove_feed(id);
        self.rss_dirty = true;
        self.rss_changed();
    }

    /// The RSS Downloader window closed: the filters run over the feeds' current items (they
    /// may have changed).
    pub fn rss_closed(&mut self) {
        let now = self.unix_now();
        let ids: Vec<FeedId> = self.rss.feeds.iter().map(|f| f.id).collect();
        let reqs: Vec<AddRequest> = ids.into_iter().flat_map(|id| self.rss.process(id, now)).collect();
        self.rss_download(reqs);
        self.rss_dirty = true;
        self.rss_changed();
    }

    /// Adds a torrent the downloader asked for: its label, "highest priority" (the top of the
    /// queue once added) and "paused state" as the Add dialog's would be.
    fn rss_add(&mut self, req: AddRequest, d: Download) {
        if let Some(key) = d.info_hashes().map(|h| h.key_hex()) {
            if self.in_list(&key).is_some() {
                self.log(tr!(LogRssAlreadyInList, title = req.title));
                return;
            }
            // picked up when the torrent first shows up in the list, like a restored label
            if !req.label.is_empty() {
                self.prefs.set(&format!("torrent.{key}.label"), &req.label);
            }
            if req.paused {
                self.prefs.set_bool(&format!("torrent.{key}.paused"), true);
            }
            if req.top_priority {
                self.queue_top.insert(key);
            }
        }
        let magnet = matches!(d, Download::Magnet(_));
        let mut p = AddTorrentParams::new(d.into_source(), self.save_path.clone());
        self.gui_options().apply_to_new(&mut p);
        req.apply_to(&mut p);
        if magnet {
            // the item's title until the metadata names it
            p.name = Some(req.title.clone());
        }
        self.add_to_session(p);
        self.save_prefs();
    }

    /// Starts the downloads `reqs` ask for (the magnet links are added at once).
    fn rss_download(&mut self, reqs: Vec<AddRequest>) {
        if reqs.is_empty() {
            return;
        }
        for r in &reqs {
            let feed = self.rss.feed(r.feed).map(|f| f.name().to_owned()).unwrap_or_default();
            self.log(tr!(RssLogDownloading, name = feed, title = r.title));
        }
        let proxy = self.proxy();
        for (req, d) in self.rss_worker.download(reqs, proxy.as_ref()) {
            self.rss_add(req, d);
        }
        self.rss_dirty = true;
    }

    // ---- what the search windows ask (D41) ---------------------------------------------------

    /// The search sources (`search.source.N.*`).
    pub fn search_sources(&self) -> Vec<SearchSource> {
        load_sources(&self.prefs)
    }

    /// Stores the search sources; returns the old ones (to remap the search box's mode).
    pub fn store_search_sources(&mut self, sources: &[SearchSource]) -> Vec<SearchSource> {
        let old = load_sources(&self.prefs);
        store_sources(&mut self.prefs, sources);
        self.save_prefs();
        old
    }

    /// The search helper's proxy, from the session's.
    pub fn helper_proxy(&self) -> Option<crate::search::helper::HelperProxy> {
        crate::search::helper::HelperProxy::from_settings(&self.options().settings)
    }
}
