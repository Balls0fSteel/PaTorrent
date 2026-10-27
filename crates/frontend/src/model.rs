//! The torrent list as a frontend shows it: a row cache fed by [`Alert::StateUpdate`], filtered
//! by a category and a name search and sorted by a column. Frontends draw [`TorrentList::view`] in a virtual list
//! and keep their selection as torrent ids, so rows can move under it.
//!
//! Built for 100,000 rows (Road to 1.0 item 5, D21): every torrent is kept in sort order
//! (`order`) and the view is that order filtered, so a category switch, an add or a remove never
//! sorts; a status update re-places only the rows whose sort key changed (taken out and merged
//! back in, O(n + k log n)), and an update that changes no sort key or filter membership leaves
//! the view as it was ([`TorrentList::view_generation`]). Only a new sort column or direction
//! sorts everything.
//!
//! [`Alert::StateUpdate`]: patorrent_core::Alert::StateUpdate

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use patorrent_core::{TorrentId, TorrentStatus};

use crate::columns::{Column, RowExtra, natural_cmp, natural_key_into};
use crate::status::{Category, status_text};

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
    /// Every row in sort order (as of the last refresh).
    order: Vec<TorrentId>,
    /// `order` filtered.
    view: Vec<TorrentId>,
    /// The view before it last changed.
    previous_view: Vec<TorrentId>,
    filter: Filter,
    /// The search box's text, lowercased: only names containing it are shown.
    search: String,
    sort: Option<(Column, bool)>,
    /// Rows to (re)place in `order` at the next refresh: new ones and those whose sort key
    /// changed.
    moved: HashSet<TorrentId>,
    /// Rows were removed since the last refresh.
    removed: bool,
    /// The sort changed: sort `order` again from scratch.
    resort: bool,
    /// The filter or a row's membership in it changed.
    refilter: bool,
    /// Something to show changed.
    dirty: bool,
    generation: u64,
    view_generation: u64,
    /// Rows not finished and rows moving data, for [`TorrentList::count`].
    unfinished: usize,
    active: usize,
}

/// A row as the sort compares it; `status` is the Status column's text when sorting by it
/// (made once per row rather than once per comparison).
struct SortRow<'a> {
    st: &'a TorrentStatus,
    extra: RowExtra<'a>,
    status: String,
}

/// The sort column's order alone.
fn column_cmp(col: Column, desc: bool, a: &SortRow<'_>, b: &SortRow<'_>) -> Ordering {
    let o = match col {
        Column::Status => a.status.cmp(&b.status),
        _ => col.cmp(a.st, b.st, a.extra, b.extra),
    };
    if desc { o.reverse() } else { o }
}

/// The list's order: the column, then (stable, deterministic ties) the name, then the id.
fn row_cmp(sort: Option<(Column, bool)>, a: &SortRow<'_>, b: &SortRow<'_>) -> Ordering {
    let Some((col, desc)) = sort else { return a.st.id.cmp(&b.st.id) };
    column_cmp(col, desc, a, b)
        .then_with(|| natural_cmp(&a.st.name, &b.st.name))
        .then_with(|| a.st.id.cmp(&b.st.id))
}

/// Sorts `rows` by name, then id. With many rows the names' [`natural_key`]s are made first
/// (in one buffer), so the sort compares bytes rather than parsing each name about 17 times.
///
/// [`natural_key`]: crate::columns::natural_key
fn sort_by_name(rows: &mut [SortRow<'_>]) {
    let by_name = |a: &SortRow<'_>, b: &SortRow<'_>| {
        natural_cmp(&a.st.name, &b.st.name).then_with(|| a.st.id.cmp(&b.st.id))
    };
    if rows.len() <= 64 {
        rows.sort_unstable_by(by_name);
        return;
    }
    let mut keys = Vec::new();
    let mut ranges = Vec::with_capacity(rows.len());
    for r in rows.iter() {
        let start = keys.len();
        ranges.push(natural_key_into(&r.st.name, &mut keys).then_some(start..keys.len()));
    }
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_unstable_by(|&i, &j| match (&ranges[i], &ranges[j]) {
        (Some(x), Some(y)) => {
            keys[x.clone()].cmp(&keys[y.clone()]).then_with(|| rows[i].st.id.cmp(&rows[j].st.id))
        }
        _ => by_name(&rows[i], &rows[j]),
    });
    // apply the permutation
    let sorted: Vec<(&TorrentStatus, RowExtra<'_>, String)> =
        order.iter().map(|&i| (rows[i].st, rows[i].extra, std::mem::take(&mut rows[i].status))).collect();
    for (r, (st, extra, status)) in rows.iter_mut().zip(sorted) {
        *r = SortRow { st, extra, status };
    }
}

fn is_active(st: &TorrentStatus) -> bool {
    Category::Active.contains(st)
}

/// Whether `name` contains `needle` (lowercase), ignoring case. An ASCII needle is matched
/// without allocating, so a keystroke over 100,000 names stays a few milliseconds.
fn name_matches(name: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if !needle.is_ascii() {
        return name.to_lowercase().contains(needle);
    }
    let (h, n) = (name.as_bytes(), needle.as_bytes());
    h.len() >= n.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

impl TorrentList {
    pub fn new() -> TorrentList {
        TorrentList::default()
    }

    /// Merges a status update. Call [`TorrentList::refresh`] afterwards.
    pub fn update(&mut self, statuses: impl IntoIterator<Item = TorrentStatus>) {
        for st in statuses {
            let id = st.id;
            match self.rows.get(&id) {
                None => {
                    self.moved.insert(id);
                }
                Some(old) => {
                    self.unfinished -= usize::from(!old.is_finished);
                    self.active -= usize::from(is_active(old));
                    if !self.resort && self.key_changed(old, &st) {
                        self.moved.insert(id);
                    }
                    if let Filter::Category(c) = self.filter {
                        self.refilter |= c.contains(old) != c.contains(&st);
                    }
                    self.refilter |= !self.search.is_empty() && old.name != st.name;
                }
            }
            self.unfinished += usize::from(!st.is_finished);
            self.active += usize::from(is_active(&st));
            self.rows.insert(id, st);
            self.dirty = true;
            self.generation += 1;
        }
    }

    /// Whether `new` may sort elsewhere than `old` (the same torrent).
    fn key_changed(&self, old: &TorrentStatus, new: &TorrentStatus) -> bool {
        let Some((col, _)) = self.sort else { return false };
        let x = self.extra(old.id);
        let o = match col {
            Column::Status => status_text(old, x.user_paused).cmp(&status_text(new, x.user_paused)),
            _ => col.cmp(old, new, x, x),
        };
        o != Ordering::Equal || old.name != new.name
    }

    /// Changes whenever the rows, labels or pause marks change (not filter or sort), so
    /// frontends can skip recounting categories when nothing happened.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Changes whenever [`TorrentList::view`] does (rows added, removed, moved or filtered), so
    /// frontends can skip mapping their selection when an update only changed cell values.
    pub fn view_generation(&self) -> u64 {
        self.view_generation
    }

    pub fn remove(&mut self, id: TorrentId) {
        if let Some(old) = self.rows.remove(&id) {
            self.unfinished -= usize::from(!old.is_finished);
            self.active -= usize::from(is_active(&old));
            self.moved.remove(&id);
            self.removed = true;
        }
        self.paused.remove(&id);
        self.labels.remove(&id);
        self.dirty = true;
        self.generation += 1;
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
        let changed = if paused { self.paused.insert(id) } else { self.paused.remove(&id) };
        if changed && self.sort.is_some_and(|(c, _)| c == Column::Status) && self.rows.contains_key(&id) {
            self.moved.insert(id);
        }
        self.dirty = true;
        self.generation += 1;
    }

    /// Sets or (with an empty string) clears a torrent's label.
    pub fn set_label(&mut self, id: TorrentId, label: &str) {
        if self.extra(id).label == label {
            return;
        }
        if label.is_empty() {
            self.labels.remove(&id);
        } else {
            self.labels.insert(id, label.to_owned());
        }
        if self.sort.is_some_and(|(c, _)| c == Column::Label) && self.rows.contains_key(&id) {
            self.moved.insert(id);
        }
        self.refilter |= matches!(self.filter, Filter::Label(_));
        self.dirty = true;
        self.generation += 1;
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

    /// How many torrents have no label (without walking every row).
    pub fn unlabeled(&self) -> usize {
        self.rows.len() - self.labels.keys().filter(|id| self.rows.contains_key(id)).count()
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
            self.refilter = true;
            self.dirty = true;
        }
    }

    /// The search text as given to [`TorrentList::set_search`], trimmed and lowercased.
    pub fn search(&self) -> &str {
        &self.search
    }

    /// Shows only torrents whose name contains `text`, ignoring case; empty shows all. It
    /// applies on top of the category or label filter.
    pub fn set_search(&mut self, text: &str) {
        let text = text.trim().to_lowercase();
        if self.search != text {
            self.search = text;
            self.refilter = true;
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
            self.resort = true;
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

    fn sort_row(&self, id: TorrentId) -> Option<SortRow<'_>> {
        let st = self.rows.get(&id)?;
        let extra = self.extra(id);
        let status = match self.sort {
            Some((Column::Status, _)) => status_text(st, extra.user_paused),
            _ => String::new(),
        };
        Some(SortRow { st, extra, status })
    }

    /// `ids` (all present in `rows`) in sort order.
    fn sorted(&self, ids: impl IntoIterator<Item = TorrentId>) -> Vec<TorrentId> {
        let mut v: Vec<SortRow<'_>> = ids.into_iter().filter_map(|id| self.sort_row(id)).collect();
        match self.sort {
            None => v.sort_unstable_by_key(|r| r.st.id),
            Some((Column::Name, desc)) => {
                sort_by_name(&mut v);
                if desc {
                    // by name descending, ties (equal names) still by id ascending
                    v.reverse();
                    let mut start = 0;
                    while start < v.len() {
                        let end = start
                            + v[start..]
                                .iter()
                                .take_while(|r| natural_cmp(&r.st.name, &v[start].st.name).is_eq())
                                .count();
                        v[start..end].reverse();
                        start = end;
                    }
                }
            }
            Some((col, desc)) => {
                // the column first, then each run of equal values by name: a column with few
                // ties (Size) makes no name keys, one with many (Down Speed while stopped) makes
                // them only for its runs
                v.sort_unstable_by(|a, b| column_cmp(col, desc, a, b));
                let mut start = 0;
                while start < v.len() {
                    let end = start
                        + v[start..]
                            .iter()
                            .take_while(|r| column_cmp(col, desc, &v[start], r).is_eq())
                            .count();
                    if end - start > 1 {
                        sort_by_name(&mut v[start..end]);
                    }
                    start = end;
                }
            }
        }
        v.into_iter().map(|r| r.st.id).collect()
    }

    /// Rebuilds the filtered, sorted view if anything changed. Returns whether it did.
    pub fn refresh(&mut self) -> bool {
        if !self.dirty && !self.refilter && !self.resort && self.moved.is_empty() && !self.removed {
            return false;
        }
        self.dirty = false;
        // every row is in `order` or in `moved` (new rows always are)
        let reorder = self.resort || !self.moved.is_empty() || self.removed;
        if reorder {
            let moved = std::mem::take(&mut self.moved);
            let (rows, mut order) = (&self.rows, std::mem::take(&mut self.order));
            if !moved.is_empty() || self.removed {
                order.retain(|id| !moved.contains(id) && rows.contains_key(id));
            }
            let moved = moved.into_iter().filter(|id| rows.contains_key(id));
            self.order = if self.resort {
                // the old order first: the sort runs faster on a reversed or barely changed one
                order.extend(moved);
                self.sorted(order)
            } else {
                let new = self.sorted(moved);
                self.merge(order, new)
            };
        }
        self.resort = false;
        self.removed = false;
        if reorder || self.refilter {
            self.refilter = false;
            let (rows, labels, search) = (&self.rows, &self.labels, self.search.as_str());
            let label = |id: &TorrentId| labels.get(id).map_or("", String::as_str);
            let found = |id: &TorrentId| name_matches(&rows[id].name, search);
            let view: Vec<TorrentId> = match &self.filter {
                Filter::Category(Category::All) if search.is_empty() => self.order.clone(),
                Filter::Category(c) => {
                    self.order.iter().copied().filter(|id| c.contains(&rows[id]) && found(id)).collect()
                }
                Filter::Label(l) => {
                    self.order.iter().copied().filter(|id| label(id) == l && found(id)).collect()
                }
            };
            if view != self.view {
                self.previous_view = std::mem::replace(&mut self.view, view);
                self.view_generation += 1;
            }
        }
        true
    }

    /// Merges `new` (sorted) into `order` (sorted, disjoint): each new row's place is found by
    /// binary search, so this is O(n + k log n) rather than a sort.
    fn merge(&self, order: Vec<TorrentId>, new: Vec<TorrentId>) -> Vec<TorrentId> {
        if new.is_empty() {
            return order;
        }
        let row = |id: TorrentId| self.sort_row(id).expect("rows in order are present");
        let mut out = Vec::with_capacity(order.len() + new.len());
        let mut from = 0;
        for id in new {
            let r = row(id);
            let at =
                from + order[from..].partition_point(|&o| row_cmp(self.sort, &row(o), &r) == Ordering::Less);
            out.extend_from_slice(&order[from..at]);
            out.push(id);
            from = at;
        }
        out.extend_from_slice(&order[from..]);
        out
    }

    /// Visible torrents in display order.
    pub fn view(&self) -> &[TorrentId] {
        &self.view
    }

    /// The view before the refresh that last changed it ([`TorrentList::view_generation`]):
    /// what a list control still shows, for carrying its selection over by torrent.
    pub fn previous_view(&self) -> &[TorrentId] {
        &self.previous_view
    }

    /// How many torrents each category holds, for the category tree (kept as rows change, not
    /// counted).
    pub fn count(&self, c: Category) -> usize {
        let n = self.rows.len();
        match c {
            Category::All => n,
            Category::Downloading => self.unfinished,
            Category::Completed => n - self.unfinished,
            Category::Active => self.active,
            Category::Inactive => n - self.active,
        }
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
        let g = l.generation();
        l.update([]);
        assert!(!l.refresh() && l.generation() == g, "an empty update changes nothing");
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
        use patorrent_core::TorrentFlags;
        let mut l = TorrentList::new();
        let mut s = st(1, "x", 0, false);
        s.flags = TorrentFlags::PAUSED;
        l.update([s]);
        let id = TorrentId::from_raw(1);
        assert_eq!(Column::Status.text(l.get(id).unwrap(), l.extra(id)), "Stopped");
        l.set_user_paused(id, true);
        assert_eq!(Column::Status.text(l.get(id).unwrap(), l.extra(id)), "Paused");
    }

    /// The view as it was built before it was kept incrementally: every row filtered, then
    /// sorted from scratch.
    fn rebuilt(l: &TorrentList) -> Vec<TorrentId> {
        let mut ids: Vec<TorrentId> = l
            .ids()
            .filter(|&id| match l.filter() {
                Filter::Category(c) => c.contains(l.get(id).unwrap()),
                Filter::Label(x) => l.extra(id).label == x,
            })
            .filter(|&id| l.get(id).unwrap().name.to_lowercase().contains(l.search()))
            .collect();
        match l.sort() {
            Some((col, desc)) => ids.sort_by(|a, b| {
                let (ra, rb) = (l.get(*a).unwrap(), l.get(*b).unwrap());
                let o = col.cmp(ra, rb, l.extra(*a), l.extra(*b));
                let o = if desc { o.reverse() } else { o };
                o.then_with(|| natural_cmp(&ra.name, &rb.name)).then_with(|| a.cmp(b))
            }),
            None => ids.sort_unstable(),
        }
        ids
    }

    /// A small deterministic PRNG (xorshift) so the test needs no crate.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    fn random_row(r: &mut Rng, raw: u64) -> TorrentStatus {
        use patorrent_core::TorrentFlags;
        let mut s =
            st(raw, &format!("t{} ep{}", r.next(5), r.next(30)), r.next(4) as u32 * 100, r.next(3) == 0);
        s.total_wanted = r.next(6) * 1000;
        s.upload_payload_rate = r.next(3) as u32;
        if r.next(2) == 0 {
            s.flags = TorrentFlags::PAUSED;
        }
        s
    }

    #[test]
    fn the_kept_view_matches_a_rebuilt_one() {
        let mut r = Rng(0x5eed_1e55);
        let mut l = TorrentList::new();
        l.update((1..=300).map(|i| random_row(&mut r, i)));
        let sorts = [
            None,
            Some((Column::Name, false)),
            Some((Column::Name, true)),
            Some((Column::Status, true)),
            Some((Column::DownSpeed, true)),
            Some((Column::Status, false)),
            Some((Column::Size, true)),
            Some((Column::Label, false)),
        ];
        let filters = [
            Filter::Category(Category::All),
            Filter::Category(Category::Downloading),
            Filter::Category(Category::Active),
            Filter::Label("x".into()),
            Filter::Label(String::new()),
        ];
        let searches = ["", "", "T1", "ep2", "t3 EP1"];
        for round in 0..400 {
            match r.next(10) {
                9 => l.set_search(searches[r.next(searches.len() as u64) as usize]),
                0 => l.set_sort(sorts[r.next(sorts.len() as u64) as usize]),
                1 => l.set_filter(filters[r.next(filters.len() as u64) as usize].clone()),
                2 => l.remove(TorrentId::from_raw(1 + r.next(400))),
                3 => l.set_label(TorrentId::from_raw(1 + r.next(400)), ["", "x", "y"][r.next(3) as usize]),
                4 => l.set_user_paused(TorrentId::from_raw(1 + r.next(400)), r.next(2) == 0),
                _ => {
                    let n = r.next(20);
                    let rows: Vec<_> = (0..n)
                        .map(|_| {
                            let raw = 1 + r.next(400);
                            random_row(&mut r, raw)
                        })
                        .collect();
                    l.update(rows);
                }
            }
            if r.next(3) != 0 {
                let generation = l.view_generation();
                let before = l.view().to_vec();
                l.refresh();
                assert_eq!(l.view(), rebuilt(&l).as_slice(), "round {round}");
                assert_eq!(l.view_generation() != generation, l.view() != before, "round {round}");
                for c in Category::ALL {
                    let n = l.ids().filter(|&id| c.contains(l.get(id).unwrap())).count();
                    assert_eq!(l.count(c), n, "round {round}: {c:?}");
                }
                let unlabeled = l.ids().filter(|&id| l.extra(id).label.is_empty()).count();
                assert_eq!(l.unlabeled(), unlabeled, "round {round}");
            }
        }
    }

    #[test]
    fn search_filters_by_name() {
        let mut l = TorrentList::new();
        l.update([
            st(1, "Ubuntu 24.04", 0, false),
            st(2, "Debian 13", 0, true),
            st(3, "Ünïcode ÄBC", 0, true),
        ]);
        l.set_search("  ubu ");
        l.refresh();
        assert_eq!(l.view(), [TorrentId::from_raw(1)]);
        l.set_search("äbc");
        l.refresh();
        assert_eq!(l.view(), [TorrentId::from_raw(3)], "non-ASCII ignores case too");
        l.set_search("e");
        l.set_filter(Filter::Category(Category::Completed));
        l.refresh();
        assert_eq!(l.view(), [TorrentId::from_raw(2), TorrentId::from_raw(3)], "on top of the category");
        // a rename moves a row into or out of the search
        l.set_search("debian");
        l.refresh();
        l.update([st(2, "Fedora", 0, true)]);
        l.refresh();
        assert!(l.view().is_empty());
        l.set_search("");
        l.refresh();
        assert_eq!(l.view().len(), 2);
        assert!(name_matches("abc", "") && !name_matches("ab", "abc") && name_matches("xABCx", "abc"));
    }

    #[test]
    fn a_value_change_keeps_the_view() {
        let mut l = TorrentList::new();
        l.update([st(1, "a", 0, false), st(2, "b", 0, false)]);
        l.set_sort(Some((Column::Name, false)));
        l.refresh();
        let g = l.view_generation();
        // the speed changes, the name (the sort key) does not
        l.update([st(1, "a", 500, false)]);
        assert!(l.refresh(), "a value to repaint");
        assert_eq!(l.view_generation(), g, "nothing moved");
        l.update([st(1, "c", 500, false)]);
        l.refresh();
        assert_ne!(l.view_generation(), g);
        assert_eq!(l.view(), [TorrentId::from_raw(2), TorrentId::from_raw(1)]);
    }
}
