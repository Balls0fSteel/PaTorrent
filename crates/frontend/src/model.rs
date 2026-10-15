//! The torrent list as a frontend shows it: a row cache fed by [`Alert::StateUpdate`], filtered
//! by a category and sorted by a column. Frontends draw [`TorrentList::view`] in a virtual list
//! and keep their selection as torrent ids, so rows can move under it.
//!
//! [`Alert::StateUpdate`]: libretorrent_core::Alert::StateUpdate

use std::collections::{HashMap, HashSet};

use libretorrent_core::{TorrentId, TorrentStatus};

use crate::columns::{Column, RowExtra, natural_cmp};
use crate::status::Category;

/// What the category tree selected.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Filter {
    Category(Category),
    /// Torrents with this label; the empty label means "no label".
    Label(String),
}

impl Default for Filter {
    fn default() -> Filter {
        Filter::Category(Category::All)
    }
}

#[derive(Default)]
pub struct TorrentList {
    rows: HashMap<TorrentId, TorrentStatus>,
    /// Torrents the user paused (rather than stopped).
    paused: HashSet<TorrentId>,
    labels: HashMap<TorrentId, String>,
    view: Vec<TorrentId>,
    filter: Filter,
    sort: Option<(Column, bool)>,
    dirty: bool,
}

impl TorrentList {
    pub fn new() -> TorrentList {
        TorrentList::default()
    }

    /// Merges a status update. Call [`TorrentList::refresh`] afterwards.
    pub fn update(&mut self, statuses: impl IntoIterator<Item = TorrentStatus>) {
        for st in statuses {
            self.rows.insert(st.id, st);
        }
        self.dirty = true;
    }

    pub fn remove(&mut self, id: TorrentId) {
        self.rows.remove(&id);
        self.paused.remove(&id);
        self.labels.remove(&id);
        self.dirty = true;
    }

    pub fn get(&self, id: TorrentId) -> Option<&TorrentStatus> {
        self.rows.get(&id)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn set_user_paused(&mut self, id: TorrentId, paused: bool) {
        if paused {
            self.paused.insert(id);
        } else {
            self.paused.remove(&id);
        }
        self.dirty = true;
    }

    /// Sets or (with an empty string) clears a torrent's label.
    pub fn set_label(&mut self, id: TorrentId, label: &str) {
        if label.is_empty() {
            self.labels.remove(&id);
        } else {
            self.labels.insert(id, label.to_owned());
        }
        self.dirty = true;
    }

    /// Every label in use with its torrent count, sorted by name.
    pub fn labels(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::BTreeMap<&str, usize> = Default::default();
        for (id, l) in &self.labels {
            if self.rows.contains_key(id) {
                *m.entry(l).or_default() += 1;
            }
        }
        m.into_iter().map(|(l, n)| (l.to_owned(), n)).collect()
    }

    pub fn extra(&self, id: TorrentId) -> RowExtra<'_> {
        RowExtra {
            user_paused: self.paused.contains(&id),
            label: self.labels.get(&id).map_or("", String::as_str),
        }
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    pub fn set_filter(&mut self, filter: Filter) {
        if self.filter != filter {
            self.filter = filter;
            self.dirty = true;
        }
    }

    /// `(column, descending)`; `None` keeps the order torrents arrived in.
    pub fn sort(&self) -> Option<(Column, bool)> {
        self.sort
    }

    pub fn set_sort(&mut self, sort: Option<(Column, bool)>) {
        if self.sort != sort {
            self.sort = sort;
            self.dirty = true;
        }
    }

    /// µTorrent's header click: a new column starts in its natural direction, the same column
    /// flips.
    pub fn toggle_sort(&mut self, col: Column) {
        let next = match self.sort {
            Some((c, desc)) if c == col => (col, !desc),
            _ => (col, col.default_descending()),
        };
        self.set_sort(Some(next));
    }

    /// Rebuilds the filtered, sorted view if anything changed. Returns whether it did.
    pub fn refresh(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        let (rows, paused, labels) = (&self.rows, &self.paused, &self.labels);
        let label = |id: &TorrentId| labels.get(id).map_or("", String::as_str);
        let extra = |id: &TorrentId| RowExtra { user_paused: paused.contains(id), label: label(id) };
        self.view.clear();
        self.view.extend(
            rows.values()
                .filter(|st| match &self.filter {
                    Filter::Category(c) => c.contains(st),
                    Filter::Label(l) => label(&st.id) == l,
                })
                .map(|st| st.id),
        );
        match self.sort {
            Some((col, desc)) => self.view.sort_by(|a, b| {
                let (ra, rb) = (&rows[a], &rows[b]);
                let o = col.cmp(ra, rb, extra(a), extra(b));
                let o = if desc { o.reverse() } else { o };
                // Stable, deterministic ties: by name, then by id.
                o.then_with(|| natural_cmp(&ra.name, &rb.name)).then_with(|| a.cmp(b))
            }),
            None => self.view.sort_unstable(),
        }
        true
    }

    /// Visible torrents in display order.
    pub fn view(&self) -> &[TorrentId] {
        &self.view
    }

    /// How many torrents each category holds, for the category tree.
    pub fn count(&self, c: Category) -> usize {
        self.rows.values().filter(|st| c.contains(st)).count()
    }

    pub fn ids(&self) -> impl Iterator<Item = TorrentId> + '_ {
        self.rows.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(raw: u64, name: &str, rate: u32, finished: bool) -> TorrentStatus {
        TorrentStatus {
            id: TorrentId::from_raw(raw),
            name: name.into(),
            download_payload_rate: rate,
            is_finished: finished,
            ..TorrentStatus::default()
        }
    }

    #[test]
    fn filter_sort_and_remove() {
        let mut l = TorrentList::new();
        l.update([st(1, "b ep10", 0, false), st(2, "b ep2", 50, false), st(3, "a", 10, true)]);
        assert!(l.refresh());
        assert!(!l.refresh(), "nothing changed");
        assert_eq!(l.view().len(), 3);

        l.toggle_sort(Column::Name);
        l.refresh();
        let names: Vec<_> = l.view().iter().map(|id| l.get(*id).unwrap().name.as_str()).collect();
        assert_eq!(names, ["a", "b ep2", "b ep10"]);

        l.toggle_sort(Column::DownSpeed);
        assert_eq!(l.sort(), Some((Column::DownSpeed, true)), "speed starts descending");
        l.refresh();
        assert_eq!(l.view()[0], TorrentId::from_raw(2));
        l.toggle_sort(Column::DownSpeed);
        l.refresh();
        assert_eq!(l.view()[0], TorrentId::from_raw(1));

        l.set_filter(Filter::Category(Category::Completed));
        l.refresh();
        assert_eq!(l.view(), [TorrentId::from_raw(3)]);

        l.set_label(TorrentId::from_raw(1), "linux");
        l.set_filter(Filter::Label("linux".into()));
        l.refresh();
        assert_eq!(l.view(), [TorrentId::from_raw(1)]);
        assert_eq!(l.labels(), [("linux".to_owned(), 1)]);
        l.set_filter(Filter::Category(Category::Completed));
        assert_eq!(l.count(Category::Downloading), 2);
        assert_eq!(l.count(Category::Active), 2);

        l.remove(TorrentId::from_raw(3));
        l.refresh();
        assert!(l.view().is_empty());
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn user_pause_shows_in_status() {
        use libretorrent_core::TorrentFlags;
        let mut l = TorrentList::new();
        let mut s = st(1, "x", 0, false);
        s.flags = TorrentFlags::PAUSED;
        l.update([s]);
        let id = TorrentId::from_raw(1);
        assert_eq!(Column::Status.text(l.get(id).unwrap(), l.extra(id)), "Stopped");
        l.set_user_paused(id, true);
        assert_eq!(Column::Status.text(l.get(id).unwrap(), l.extra(id)), "Paused");
    }
}
