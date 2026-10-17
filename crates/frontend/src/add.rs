//! The Add New Torrent dialog's model: the torrent's files as a tree with check boxes and
//! priorities behind a virtual list (only the visible rows are materialized, so torrents with
//! many thousands of files open at once), the recent save folders, and what the user chose,
//! turned into [`AddTorrentParams`]. Toolkit-independent, so the GTK dialog can share it.

use std::collections::BTreeMap;

use patorrent_core::metainfo::TorrentInfo;
use patorrent_core::{AddTorrentParams, DownloadPriority, TorrentFlags};

use crate::prefs::Prefs;

/// A row's check box: folders are mixed when only some of their files are wanted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    Off,
    On,
    Mixed,
}

const NO_PARENT: u32 = u32::MAX;

#[derive(Debug)]
struct Node {
    name: String,
    parent: u32,
    depth: u16,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    /// Index into the torrent's files.
    File(u32),
    Dir {
        children: Vec<u32>,
        expanded: bool,
        /// Summaries of the files below, kept by [`FileTree::recount`].
        files: u32,
        wanted: u32,
        size: u64,
        /// The one priority of its wanted files, `None` when they differ (or none is wanted).
        priority: Option<DownloadPriority>,
    },
}

/// One visible row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row<'a> {
    pub name: &'a str,
    /// Nesting level, 0 for the torrent's top entries.
    pub depth: u16,
    /// `Some(expanded)` for folders.
    pub folder: Option<bool>,
    pub check: Check,
    /// The file's size, or the sum of the folder's files.
    pub size: u64,
    /// `None` for a folder whose files differ.
    pub priority: Option<DownloadPriority>,
    /// The torrent file index, for files.
    pub file: Option<usize>,
}

/// The torrent's files as a tree. Folders come before the files of their parent in the order
/// the torrent lists them; pad files are left out (and never downloaded).
#[derive(Debug)]
pub struct FileTree {
    nodes: Vec<Node>,
    /// Top-level nodes.
    roots: Vec<u32>,
    sizes: Vec<u64>,
    /// Per torrent file: `None` for pad files.
    node_of: Vec<Option<u32>>,
    wanted: Vec<bool>,
    /// The priority a file gets while it is wanted (kept when it is unchecked and checked again).
    priority: Vec<DownloadPriority>,
    /// Visible node indices, in display order.
    rows: Vec<u32>,
}

impl FileTree {
    pub fn from_torrent(ti: &TorrentInfo) -> FileTree {
        let fs = ti.files();
        FileTree::new((0..fs.num_files()).map(|i| {
            let path = fs.file_path(i, "");
            let mut parts: Vec<String> =
                path.split(['/', '\\']).filter(|s| !s.is_empty()).map(str::to_owned).collect();
            // multi-file torrents: the first component is the torrent's own folder
            if parts.len() > 1 {
                parts.remove(0);
            }
            (parts, fs.file_size(i), fs.pad_file_at(i))
        }))
    }

    /// `files` are (path components below the torrent's folder, size, is a pad file), in torrent
    /// order. Every folder starts expanded and every file wanted at normal priority.
    pub fn new(files: impl IntoIterator<Item = (Vec<String>, u64, bool)>) -> FileTree {
        let mut t = FileTree {
            nodes: Vec::new(),
            roots: Vec::new(),
            sizes: Vec::new(),
            node_of: Vec::new(),
            wanted: Vec::new(),
            priority: Vec::new(),
            rows: Vec::new(),
        };
        // (parent node, name) -> folder node
        let mut dirs: std::collections::HashMap<(u32, String), u32> = std::collections::HashMap::new();
        for (i, (parts, size, pad)) in files.into_iter().enumerate() {
            t.sizes.push(size);
            t.wanted.push(!pad);
            t.priority.push(DownloadPriority::NORMAL);
            if pad || parts.is_empty() {
                t.node_of.push(None);
                continue;
            }
            let mut parent = NO_PARENT;
            let (file_name, folders) = parts.split_last().expect("not empty");
            for (depth, folder) in folders.iter().enumerate() {
                parent = match dirs.get(&(parent, folder.clone())) {
                    Some(&n) => n,
                    None => {
                        let n = t.push(
                            parent,
                            Node {
                                name: folder.clone(),
                                parent,
                                depth: depth as u16,
                                kind: Kind::Dir {
                                    children: Vec::new(),
                                    expanded: true,
                                    files: 0,
                                    wanted: 0,
                                    size: 0,
                                    priority: None,
                                },
                            },
                        );
                        dirs.insert((parent, folder.clone()), n);
                        n
                    }
                };
            }
            let n = t.push(
                parent,
                Node {
                    name: file_name.clone(),
                    parent,
                    depth: folders.len() as u16,
                    kind: Kind::File(i as u32),
                },
            );
            t.node_of.push(Some(n));
        }
        t.recount();
        t.rebuild_rows();
        t
    }

    fn push(&mut self, parent: u32, node: Node) -> u32 {
        let n = self.nodes.len() as u32;
        self.nodes.push(node);
        if parent == NO_PARENT {
            self.roots.push(n);
        } else if let Kind::Dir { children, .. } = &mut self.nodes[parent as usize].kind {
            children.push(n);
        }
        n
    }

    /// Folder summaries. Every child has a higher index than its folder, so walking backwards
    /// finishes each folder before it is added to its own parent.
    fn recount(&mut self) {
        for node in &mut self.nodes {
            if let Kind::Dir { files, wanted, size, priority, .. } = &mut node.kind {
                (*files, *wanted, *size, *priority) = (0, 0, 0, None);
            }
        }
        // per folder: the wanted files' priorities differ
        let mut differ = vec![false; self.nodes.len()];
        for i in (0..self.nodes.len()).rev() {
            let (f, w, s, p) = match &mut self.nodes[i].kind {
                Kind::File(fi) => {
                    let fi = *fi as usize;
                    let on = self.wanted[fi];
                    (1, on as u32, self.sizes[fi], on.then_some(self.priority[fi]))
                }
                Kind::Dir { files, wanted, size, priority, .. } => {
                    if differ[i] {
                        *priority = None;
                    }
                    (*files, *wanted, *size, *priority)
                }
            };
            let parent = self.nodes[i].parent;
            if parent == NO_PARENT {
                continue;
            }
            let (had_wanted, mixed_below) = (w > 0, w > 0 && p.is_none());
            if let Kind::Dir { files, wanted, size, priority, .. } = &mut self.nodes[parent as usize].kind {
                if had_wanted {
                    if mixed_below || (*wanted > 0 && *priority != p) {
                        differ[parent as usize] = true;
                    }
                    if *wanted == 0 {
                        *priority = p;
                    }
                }
                *files += f;
                *wanted += w;
                *size += s;
            }
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        let mut stack: Vec<u32> = self.roots.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            self.rows.push(n);
            if let Kind::Dir { children, expanded: true, .. } = &self.nodes[n as usize].kind {
                stack.extend(children.iter().rev());
            }
        }
    }

    /// Visible rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn row(&self, i: usize) -> Option<Row<'_>> {
        let node = &self.nodes[*self.rows.get(i)? as usize];
        Some(match &node.kind {
            &Kind::File(fi) => {
                let fi = fi as usize;
                Row {
                    name: &node.name,
                    depth: node.depth,
                    folder: None,
                    check: if self.wanted[fi] { Check::On } else { Check::Off },
                    size: self.sizes[fi],
                    priority: Some(if self.wanted[fi] {
                        self.priority[fi]
                    } else {
                        DownloadPriority::DONT_DOWNLOAD
                    }),
                    file: Some(fi),
                }
            }
            Kind::Dir { expanded, files, wanted, size, priority, .. } => Row {
                name: &node.name,
                depth: node.depth,
                folder: Some(*expanded),
                check: match *wanted {
                    0 => Check::Off,
                    w if w == *files => Check::On,
                    _ => Check::Mixed,
                },
                size: *size,
                priority: if *wanted == 0 {
                    Some(DownloadPriority::DONT_DOWNLOAD)
                } else if *wanted == *files {
                    *priority
                } else {
                    None
                },
                file: None,
            },
        })
    }

    /// Expands or collapses the folder in row `i`; false for files.
    pub fn toggle_expanded(&mut self, i: usize) -> bool {
        self.set_expanded(i, None)
    }

    /// Expands (`Some(true)`), collapses (`Some(false)`) or toggles (`None`) the folder in row
    /// `i`; true if it changed.
    pub fn set_expanded(&mut self, i: usize, to: Option<bool>) -> bool {
        let Some(&n) = self.rows.get(i) else { return false };
        let Kind::Dir { expanded, .. } = &mut self.nodes[n as usize].kind else { return false };
        let new = to.unwrap_or(!*expanded);
        if new == *expanded {
            return false;
        }
        *expanded = new;
        self.rebuild_rows();
        true
    }

    /// The row of the folder holding row `i`.
    pub fn parent_row(&self, i: usize) -> Option<usize> {
        let parent = self.nodes[*self.rows.get(i)? as usize].parent;
        (parent != NO_PARENT).then(|| self.rows.iter().position(|&r| r == parent)).flatten()
    }

    /// The torrent files in row `i` (the folder's files, recursively).
    fn files_of(&self, i: usize, out: &mut Vec<usize>) {
        let Some(&n) = self.rows.get(i) else { return };
        let mut stack = vec![n];
        while let Some(n) = stack.pop() {
            match &self.nodes[n as usize].kind {
                Kind::File(fi) => out.push(*fi as usize),
                Kind::Dir { children, .. } => stack.extend(children),
            }
        }
    }

    /// Clicking the check box of the given rows: a row that is fully on turns off, anything
    /// else turns on. With several rows, the first decides for all of them.
    pub fn toggle_checks(&mut self, rows: &[usize]) {
        let Some(first) = rows.first().and_then(|&r| self.row(r)) else { return };
        let on = first.check != Check::On;
        self.set_checks(rows, on);
    }

    pub fn set_checks(&mut self, rows: &[usize], on: bool) {
        let mut files = Vec::new();
        for &r in rows {
            self.files_of(r, &mut files);
        }
        for fi in files {
            self.wanted[fi] = on;
        }
        self.recount();
    }

    /// Select All / Select None.
    pub fn set_all(&mut self, on: bool) {
        for (fi, node) in self.node_of.iter().enumerate() {
            if node.is_some() {
                self.wanted[fi] = on;
            }
        }
        self.recount();
    }

    /// Sets the priority of the rows' files; "Don't Download" unchecks them, anything else
    /// checks them.
    pub fn set_priority(&mut self, rows: &[usize], p: DownloadPriority) {
        let mut files = Vec::new();
        for &r in rows {
            self.files_of(r, &mut files);
        }
        for fi in files {
            if p == DownloadPriority::DONT_DOWNLOAD {
                self.wanted[fi] = false;
            } else {
                self.wanted[fi] = true;
                self.priority[fi] = p;
            }
        }
        self.recount();
    }

    /// One priority per torrent file, 0 for unchecked and pad files (`AddTorrentParams`).
    pub fn file_priorities(&self) -> Vec<DownloadPriority> {
        (0..self.sizes.len())
            .map(|fi| {
                if self.wanted[fi] && self.node_of[fi].is_some() {
                    self.priority[fi]
                } else {
                    DownloadPriority::DONT_DOWNLOAD
                }
            })
            .collect()
    }

    /// Bytes of the wanted files and of all files (pad files left out).
    pub fn sizes(&self) -> (u64, u64) {
        let mut sel = 0;
        let mut all = 0;
        for (fi, node) in self.node_of.iter().enumerate() {
            if node.is_some() {
                all += self.sizes[fi];
                if self.wanted[fi] {
                    sel += self.sizes[fi];
                }
            }
        }
        (sel, all)
    }

    /// Torrent files that are not pad files.
    pub fn num_files(&self) -> usize {
        self.node_of.iter().filter(|n| n.is_some()).count()
    }
}

/// What the user chose in the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddChoice {
    pub save_path: String,
    /// A new name, when it differs from the torrent's.
    pub name: Option<String>,
    pub file_priorities: Vec<DownloadPriority>,
    /// "Start torrent": queued and auto-managed; off adds it stopped.
    pub start: bool,
    /// "Skip hash check": libtorrent's seed mode (the files are assumed complete).
    pub skip_hash_check: bool,
    /// "Add to top of queue".
    pub top_of_queue: bool,
    /// Empty for no label.
    pub label: String,
}

impl AddChoice {
    /// Fills in the parts of `p` the dialog decides (after the Preferences' defaults). A new
    /// name renames the torrent's folder (or its only file) on disk too.
    pub fn apply_to(&self, ti: &TorrentInfo, p: &mut AddTorrentParams) {
        p.save_path = self.save_path.clone();
        p.name = self.name.clone();
        p.renamed_files = self.name.as_deref().map(|n| renamed_root(ti, n)).unwrap_or_default();
        p.file_priorities = self.file_priorities.clone();
        if self.start {
            p.flags |= TorrentFlags::AUTO_MANAGED | TorrentFlags::PAUSED;
        } else {
            // µTorrent's Stop: paused and outside the queue
            p.flags = p.flags & !TorrentFlags::AUTO_MANAGED | TorrentFlags::PAUSED;
        }
        p.flags.set(TorrentFlags::SEED_MODE, self.skip_hash_check);
    }
}

/// The Add New Torrent dialog's "Name" as file renames (libtorrent `renamed_files`): a
/// multi-file torrent's folder, or a single-file torrent's file, stored under `name`. Empty
/// when `name` is the torrent's own or cannot be one path component (a separator, `.`, `..`).
pub fn renamed_root(ti: &TorrentInfo, name: &str) -> BTreeMap<u32, String> {
    let fs = ti.files();
    let (name, root) = (name.trim(), fs.name());
    let mut out = BTreeMap::new();
    if name.is_empty() || name == root || name == "." || name == ".." || name.contains(['/', '\\']) {
        return out;
    }
    for i in 0..fs.num_files() {
        let path = fs.file_path(i, "");
        let renamed = match path.strip_prefix(root) {
            Some("") => name.to_owned(),
            Some(rest) if rest.starts_with('/') => format!("{name}{rest}"),
            // absolute or outside the torrent's folder: nothing to rename
            _ => continue,
        };
        out.insert(i as u32, renamed);
    }
    out
}

/// How many save folders the Save In list remembers.
pub const RECENT_DIRS: usize = 10;
const RECENT_KEY: &str = "add.recent_dirs";

/// The remembered save folders, most recent first (`add.recent_dirs`, `|`-separated: no
/// Windows or Unix path in practice contains one).
pub fn recent_dirs(prefs: &Prefs) -> Vec<String> {
    prefs
        .get(RECENT_KEY)
        .unwrap_or_default()
        .split('|')
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// Puts `dir` first in the remembered folders.
pub fn remember_dir(prefs: &mut Prefs, dir: &str) {
    let dir = dir.trim();
    if dir.is_empty() || dir.contains('|') {
        return;
    }
    let mut dirs = recent_dirs(prefs);
    dirs.retain(|d| !same_dir(d, dir));
    dirs.insert(0, dir.to_owned());
    dirs.truncate(RECENT_DIRS);
    prefs.set(RECENT_KEY, dirs.join("|"));
}

/// The Save In list: the default download folder first, then the recent ones.
pub fn save_in_choices(default_dir: &str, recent: &[String]) -> Vec<String> {
    let mut out = vec![default_dir.to_owned()];
    for d in recent {
        if !out.iter().any(|o| same_dir(o, d)) {
            out.push(d.clone());
        }
    }
    out
}

/// Folder names compare without case and trailing separators (Windows paths).
fn same_dir(a: &str, b: &str) -> bool {
    let t = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_lowercase();
    t(a) == t(b)
}

/// "1.20 GB of 3.40 GB selected" (or just the size when everything is).
pub fn size_text(selected: u64, total: u64) -> String {
    if selected == total {
        crate::fmt::size(total)
    } else {
        format!("{} of {} selected", crate::fmt::size(selected), crate::fmt::size(total))
    }
}

#[cfg(test)]
mod tests {
    use patorrent_core::TorrentSource;

    use super::*;

    fn p(s: &str) -> Vec<String> {
        s.split('/').map(str::to_owned).collect()
    }

    /// a.txt, dir/b.bin, dir/sub/c.bin, (pad), dir/d.bin, e.iso
    fn sample() -> FileTree {
        FileTree::new([
            (p("a.txt"), 10, false),
            (p("dir/b.bin"), 100, false),
            (p("dir/sub/c.bin"), 1000, false),
            (p(".pad/5"), 5, true),
            (p("dir/d.bin"), 10_000, false),
            (p("e.iso"), 100_000, false),
        ])
    }

    fn names(t: &FileTree) -> Vec<String> {
        (0..t.len()).map(|i| t.row(i).unwrap().name.to_owned()).collect()
    }

    #[test]
    fn builds_the_tree_in_torrent_order() {
        let t = sample();
        assert_eq!(names(&t), ["a.txt", "dir", "b.bin", "sub", "c.bin", "d.bin", "e.iso"]);
        let dir = t.row(1).unwrap();
        assert_eq!((dir.folder, dir.depth, dir.size, dir.check), (Some(true), 0, 11_100, Check::On));
        assert_eq!(dir.priority, Some(DownloadPriority::NORMAL));
        assert_eq!(t.row(4).unwrap().depth, 2);
        assert_eq!(t.row(4).unwrap().file, Some(2));
        assert_eq!(t.parent_row(4), Some(3));
        assert_eq!(t.parent_row(0), None);
        assert_eq!(t.sizes(), (111_110, 111_110));
        assert_eq!(t.num_files(), 5);
        // the pad file is never downloaded
        assert_eq!(t.file_priorities()[3], DownloadPriority::DONT_DOWNLOAD);
        assert_eq!(t.file_priorities().iter().filter(|p| p.0 == 4).count(), 5);
    }

    #[test]
    fn folders_toggle_their_files() {
        let mut t = sample();
        t.toggle_checks(&[1]);
        let prios: Vec<u8> = t.file_priorities().iter().map(|p| p.0).collect();
        assert_eq!(prios, [4, 0, 0, 0, 0, 4]);
        assert_eq!(t.row(1).unwrap().check, Check::Off);
        assert_eq!(t.row(3).unwrap().check, Check::Off);
        assert_eq!(t.sizes(), (100_010, 111_110));

        // one file back on: the folders above it are mixed
        t.toggle_checks(&[4]);
        assert_eq!(t.row(4).unwrap().check, Check::On);
        assert_eq!(t.row(3).unwrap().check, Check::On);
        assert_eq!(t.row(1).unwrap().check, Check::Mixed);
        assert_eq!(t.row(1).unwrap().priority, None);
        // a mixed folder turns on
        t.toggle_checks(&[1]);
        assert_eq!(t.row(1).unwrap().check, Check::On);

        t.set_all(false);
        assert!(t.file_priorities().iter().all(|p| p.0 == 0));
        assert_eq!(t.sizes().0, 0);
        t.set_all(true);
        assert_eq!(t.file_priorities()[3].0, 0);
        assert_eq!(t.sizes().0, 111_110);
    }

    #[test]
    fn priorities_survive_unchecking() {
        let mut t = sample();
        t.set_priority(&[3], DownloadPriority::TOP);
        assert_eq!(t.row(3).unwrap().priority, Some(DownloadPriority::TOP));
        assert_eq!(t.row(1).unwrap().priority, None, "b.bin and d.bin stay normal");
        t.set_priority(&[1], DownloadPriority::LOW);
        assert_eq!(t.row(1).unwrap().priority, Some(DownloadPriority::LOW));
        t.set_priority(&[4], DownloadPriority::TOP);
        t.toggle_checks(&[4]);
        assert_eq!(t.file_priorities()[2].0, 0);
        assert_eq!(t.row(4).unwrap().priority, Some(DownloadPriority::DONT_DOWNLOAD));
        t.toggle_checks(&[4]);
        assert_eq!(t.file_priorities()[2].0, 7);
        t.set_priority(&[0], DownloadPriority::DONT_DOWNLOAD);
        assert_eq!(t.row(0).unwrap().check, Check::Off);
    }

    #[test]
    fn collapsing_hides_the_folder_contents() {
        let mut t = sample();
        assert!(t.toggle_expanded(3));
        assert_eq!(names(&t), ["a.txt", "dir", "b.bin", "sub", "d.bin", "e.iso"]);
        assert!(t.toggle_expanded(1));
        assert_eq!(names(&t), ["a.txt", "dir", "e.iso"]);
        assert!(!t.toggle_expanded(0), "files do not expand");
        assert!(!t.set_expanded(1, Some(false)));
        // the inner folder stays collapsed inside the reopened one
        assert!(t.set_expanded(1, Some(true)));
        assert_eq!(names(&t), ["a.txt", "dir", "b.bin", "sub", "d.bin", "e.iso"]);
        // a collapsed folder still toggles every file below it
        t.toggle_checks(&[3]);
        assert_eq!(t.file_priorities()[2].0, 0);
    }

    #[test]
    fn thousands_of_files() {
        let files = (0..20_000).map(|i| (p(&format!("d{}/s{}/f{i}", i % 10, i % 7)), 1u64, false));
        let start = std::time::Instant::now();
        let mut t = FileTree::new(files);
        assert_eq!(t.num_files(), 20_000);
        assert_eq!(t.len(), 20_000 + 10 + 70);
        t.toggle_checks(&[0]);
        assert_eq!(t.sizes(), (18_000, 20_000));
        assert!(start.elapsed().as_secs() < 5);
    }

    #[test]
    fn single_and_multi_file_torrents() {
        // one file: its name is the only row
        let single =
            b"d4:infod6:lengthi5e4:name5:a.iso12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        let t = FileTree::from_torrent(&TorrentInfo::from_bytes(single).unwrap());
        assert_eq!(names(&t), ["a.iso"]);
        let multi = b"d4:infod5:filesld6:lengthi3e4:pathl1:x5:y.txteed6:lengthi4e4:pathl5:z.txteee4:name4:root12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        let t = FileTree::from_torrent(&TorrentInfo::from_bytes(multi).unwrap());
        assert_eq!(names(&t), ["x", "y.txt", "z.txt"]);
        assert_eq!(t.sizes(), (7, 7));
    }

    #[test]
    fn the_choice_becomes_add_params() {
        let mut params = AddTorrentParams::new(TorrentSource::Magnet(String::new()), "old");
        params.flags = TorrentFlags::DEFAULT;
        let mut c = AddChoice {
            save_path: r"D:\in".into(),
            name: Some("New".into()),
            file_priorities: vec![DownloadPriority(0), DownloadPriority(7)],
            start: false,
            skip_hash_check: true,
            top_of_queue: true,
            label: "Linux".into(),
        };
        let multi = b"d4:infod5:filesld6:lengthi3e4:pathl1:x5:y.txteed6:lengthi4e4:pathl5:z.txteee4:name4:root12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        let ti = TorrentInfo::from_bytes(multi).unwrap();
        c.apply_to(&ti, &mut params);
        assert_eq!(params.save_path, r"D:\in");
        assert_eq!(params.name.as_deref(), Some("New"));
        let renamed: Vec<_> = params.renamed_files.values().map(String::as_str).collect();
        assert_eq!(renamed, ["New/x/y.txt", "New/z.txt"]);
        assert_eq!(params.file_priorities, c.file_priorities);
        assert!(params.flags.contains(TorrentFlags::PAUSED | TorrentFlags::SEED_MODE));
        assert!(!params.flags.contains(TorrentFlags::AUTO_MANAGED));
        c.start = true;
        c.skip_hash_check = false;
        c.name = None;
        c.apply_to(&ti, &mut params);
        assert!(params.renamed_files.is_empty());
        assert!(params.flags.contains(TorrentFlags::DEFAULT));
        assert!(!params.flags.contains(TorrentFlags::SEED_MODE));
    }

    #[test]
    fn a_new_name_renames_the_folder_or_the_file() {
        let single =
            b"d4:infod6:lengthi5e4:name5:a.iso12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        let ti = TorrentInfo::from_bytes(single).unwrap();
        assert_eq!(renamed_root(&ti, " b.iso "), BTreeMap::from([(0, "b.iso".to_owned())]));
        for same_or_bad in ["a.iso", "", "..", "x/y", r"x\y"] {
            assert!(renamed_root(&ti, same_or_bad).is_empty(), "{same_or_bad}");
        }
        let multi = b"d4:infod5:filesld6:lengthi3e4:pathl1:x5:y.txteed6:lengthi4e4:pathl5:z.txteee4:name4:root12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        let ti = TorrentInfo::from_bytes(multi).unwrap();
        let r = renamed_root(&ti, "Other");
        assert_eq!(r, BTreeMap::from([(0, "Other/x/y.txt".to_owned()), (1, "Other/z.txt".to_owned())]));
        assert!(renamed_root(&ti, "root").is_empty());
    }

    #[test]
    fn recent_folders() {
        let mut prefs = Prefs::default();
        assert!(recent_dirs(&prefs).is_empty());
        remember_dir(&mut prefs, r"C:\a");
        remember_dir(&mut prefs, r"D:\b\");
        remember_dir(&mut prefs, r"c:\A");
        assert_eq!(recent_dirs(&prefs), [r"c:\A", r"D:\b\"]);
        for i in 0..20 {
            remember_dir(&mut prefs, &format!(r"E:\{i}"));
        }
        assert_eq!(recent_dirs(&prefs).len(), RECENT_DIRS);
        assert_eq!(recent_dirs(&prefs)[0], r"E:\19");
        assert_eq!(save_in_choices(r"E:\19\", &recent_dirs(&prefs))[..2], [r"E:\19\", r"E:\18"]);
        assert_eq!(size_text(5, 5), "5 B");
        assert_eq!(size_text(2048, 4096), "2.00 kB of 4.00 kB selected");
    }
}
