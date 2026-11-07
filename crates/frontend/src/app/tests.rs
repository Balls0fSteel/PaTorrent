//! `Core` on a mock session: alerts made up here go through [`Core::handle_alerts`], and the
//! engine's own (adds, flags, queue moves) come back through [`Core::on_alerts`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rookparcel_core::{
    Alert, InfoHashes, MockConfig, Session, SessionStats, Settings, Sha1Hash, TorrentFlags, TorrentId,
    TorrentState, TorrentStatus,
};

use super::*;
use crate::add::AddChoice;
use crate::scheduler::Mode;
use crate::seeding::SeedGoal;

/// A folder of the test's own.
fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rookparcel-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Monday 2026-11-02 12:00 UTC.
const NOON: i64 = 1_793_966_400;

struct Setup {
    prefs: Prefs,
    options: Options,
    mock: MockConfig,
    remote: Option<Remote>,
}

impl Setup {
    fn new() -> Setup {
        let prefs = Prefs::default();
        let options = Options::load(&prefs, "/dl");
        Setup { prefs, options, mock: MockConfig::default(), remote: None }
    }

    fn start(self, name: &str) -> Core {
        let dir = temp(name);
        let settings = Settings { tick_interval: 50, ..self.options.settings.clone() };
        let session = Session::start_mock(settings, self.mock);
        Core::start(Start {
            session,
            remote: self.remote,
            prefs: self.prefs,
            options: self.options,
            dirs: Dirs { config: dir.clone(), settings: dir.clone(), data: dir.clone(), root: dir },
            wake: Arc::new(|_| {}),
            clock: Arc::new(|| Some((NOON, 0))),
            updates: false,
        })
    }
}

fn hash(n: u8) -> InfoHashes {
    InfoHashes::v1(Sha1Hash([n; 20]))
}

/// A torrent's status as a made-up `StateUpdate` carries it.
fn status(id: u64, n: u8, name: &str, finished: bool) -> TorrentStatus {
    TorrentStatus {
        id: TorrentId::from_raw(id),
        info_hashes: hash(n),
        name: name.to_owned(),
        is_finished: finished,
        is_seeding: finished,
        state: if finished { TorrentState::Seeding } else { TorrentState::Downloading },
        flags: TorrentFlags::AUTO_MANAGED,
        ..TorrentStatus::default()
    }
}

/// A `.torrent` of 4 GiB (it never finishes in a test), named `name`.
fn torrent(name: &str) -> Vec<u8> {
    // 256 pieces of 16 MiB
    let mut b = format!(
        "d4:infod6:lengthi4294967296e4:name{}:{name}12:piece lengthi16777216e6:pieces5120:",
        name.len()
    )
    .into_bytes();
    b.extend([b'a'; 5120]);
    b.extend(b"ee");
    b
}

/// Asks the engine for the torrents' state and drains the alerts until `done` holds.
fn pump(core: &mut Core, what: &str, mut done: impl FnMut(&Core) -> bool) -> Events {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut all = Events::default();
    loop {
        core.handle.post_torrent_updates();
        std::thread::sleep(Duration::from_millis(20));
        all.merge(core.on_alerts());
        if done(core) {
            return all;
        }
        assert!(Instant::now() < deadline, "waited in vain for {what}");
    }
}

fn logged(core: &Core, text: &str) -> bool {
    core.detail().log.iter().any(|l| l.contains(text))
}

fn choice() -> AddChoice {
    AddChoice {
        save_path: "/dl".into(),
        name: None,
        file_priorities: Vec::new(),
        start: true,
        skip_hash_check: false,
        top_of_queue: false,
        label: String::new(),
    }
}

#[test]
fn first_sight_takes_the_label_and_pause_mark_from_the_preferences() {
    let mut s = Setup::new();
    let key = hash(1).key_hex();
    s.prefs.set(&format!("torrent.{key}.label"), "Linux");
    s.prefs.set_bool(&format!("torrent.{key}.paused"), true);
    let mut core = s.start("first-sight");
    core.handle_alerts(vec![Alert::StateUpdate(vec![status(1, 1, "a", false), status(2, 2, "b", false)])]);
    let ev = core.events();
    assert!(ev.rows && ev.view && ev.sidebar);
    let (a, b) = (TorrentId::from_raw(1), TorrentId::from_raw(2));
    assert_eq!(core.list().extra(a).label, "Linux");
    assert!(core.list().extra(a).user_paused);
    assert_eq!(core.list().extra(b).label, "");
    assert!(!core.list().extra(b).user_paused);
    assert_eq!(core.labels(), ["Linux"]);
    // only at first sight: a label changed meanwhile stays
    core.set_label(&[a], "Other");
    core.handle_alerts(vec![Alert::StateUpdate(vec![status(1, 1, "a", false)])]);
    assert_eq!(core.list().extra(a).label, "Other");
    assert_eq!(core.prefs().get(&format!("torrent.{key}.label")), Some("Other"));
    // Pause marks it, Start clears it
    core.torrent_action(TorrentAction::Pause, &[b]);
    assert!(core.prefs().get_bool(&format!("torrent.{}.paused", hash(2).key_hex()), false));
    core.torrent_action(TorrentAction::Start, &[b]);
    assert!(core.prefs().get(&format!("torrent.{}.paused", hash(2).key_hex())).is_none());
}

#[test]
fn a_removed_torrent_takes_its_preferences_along() {
    let mut s = Setup::new();
    let key = hash(3).key_hex();
    s.prefs.set(&format!("torrent.{key}.label"), "x");
    s.prefs.set_bool(&format!("torrent.{key}.paused"), true);
    let mut core = s.start("removed");
    let id = TorrentId::from_raw(3);
    core.handle_alerts(vec![Alert::StateUpdate(vec![status(3, 3, "c", false)])]);
    core.select(&[id]);
    assert_eq!(core.detail().id, Some(id));
    core.handle_alerts(vec![Alert::TorrentRemoved { id, info_hashes: hash(3) }]);
    assert!(core.list().get(id).is_none());
    assert!(core.prefs().get(&format!("torrent.{key}.label")).is_none());
    assert!(core.prefs().get(&format!("torrent.{key}.paused")).is_none());
    assert_eq!(core.detail().id, None);
    assert!(core.events().view);
}

#[test]
fn only_a_download_seen_running_is_reported_completed() {
    let mut core = Setup::new().start("finished");
    let (dl, seed) = (TorrentId::from_raw(4), TorrentId::from_raw(5));
    core.handle_alerts(vec![Alert::StateUpdate(vec![
        status(4, 4, "download", false),
        status(5, 5, "seed", true),
    ])]);
    core.events();
    core.handle_alerts(vec![Alert::TorrentFinished { id: dl }, Alert::TorrentFinished { id: seed }]);
    let ev = core.events();
    assert_eq!(ev.completed, ["download"]);
    assert!(logged(&core, &tr!(LogFinished, name = "seed")));
    // once
    core.handle_alerts(vec![Alert::TorrentFinished { id: dl }]);
    assert!(core.events().completed.is_empty());
    // without "Show balloon notifications" nothing is reported
    core.prefs_mut().set_bool("ui.balloons", false);
    core.handle_alerts(vec![Alert::StateUpdate(vec![status(6, 6, "quiet", false)])]);
    core.handle_alerts(vec![Alert::TorrentFinished { id: TorrentId::from_raw(6) }]);
    assert!(core.events().completed.is_empty());
}

#[test]
fn add_to_top_of_queue_moves_it_once_added() {
    let mut core = Setup::new().start("top");
    for name in ["one", "two"] {
        match core.add_file(torrent(name)) {
            AddStep::Dialog(r) => assert!(core.finish_add(r, Some(choice())).is_none()),
            other => panic!("{other:?}"),
        }
    }
    pump(&mut core, "two torrents", |c| c.list().len() == 2);
    let AddStep::Dialog(r) = core.add_file(torrent("three")) else { panic!("no dialog") };
    assert!(!r.is_magnet());
    assert_eq!(r.name(), "three");
    let key = r.key.clone();
    core.finish_add(r, Some(AddChoice { top_of_queue: true, label: "L".into(), ..choice() }));
    pump(&mut core, "the third at the top", |c| {
        c.list()
            .ids()
            .filter_map(|id| c.list().get(id))
            .any(|s| s.name == "three" && s.queue_position == Some(0))
    });
    let id = core.list().ids().find(|&id| core.list().get(id).unwrap().name == "three").unwrap();
    assert_eq!(core.list().extra(id).label, "L");
    assert!(logged(&core, &tr!(LogAdded, name = "three")));
    assert!(core.queue_top.is_empty());
    // the same torrent again is refused
    match core.add_file(torrent("three")) {
        AddStep::AlreadyInList(name) => assert_eq!(name, "three"),
        other => panic!("{other:?}"),
    }
    assert!(core.prefs().get(&format!("torrent.{key}.label")).is_some());
}

#[test]
fn a_magnet_waits_for_its_metadata_then_opens_its_dialog() {
    let mut core = Setup::new().start("magnet");
    let hex = "0123456789abcdef0123456789abcdef01234567";
    let step = core.add_url(&format!("  magnet:?xt=urn:btih:{hex}  "));
    assert!(matches!(step, AddStep::Awaiting), "{step:?}");
    assert_eq!(core.prefs().get(&format!("torrent.{hex}.awaiting")), Some("/dl"));
    assert!(core.next_magnet_dialog().is_none());
    let ev = pump(&mut core, "the metadata", |c| !c.awaiting.ready().is_empty());
    assert!(ev.magnet_ready);
    let r = core.next_magnet_dialog().expect("a dialog");
    assert!(r.is_magnet() && r.setup.in_session);
    assert_eq!(r.key, hex);
    // one at a time
    assert!(core.next_magnet_dialog().is_none());
    assert!(core.finish_magnet(r, Some(AddChoice { label: "M".into(), ..choice() })).is_none());
    assert!(core.prefs().get(&format!("torrent.{hex}.awaiting")).is_none());
    assert_eq!(core.prefs().get(&format!("torrent.{hex}.label")), Some("M"));
    pump(&mut core, "the magnet out of upload mode", |c| {
        c.list().ids().filter_map(|id| c.list().get(id)).any(|s| !s.flags.contains(TorrentFlags::UPLOAD_MODE))
    });
    // in the list now: the link again is refused, and text that is no link is an error
    assert!(matches!(core.add_magnet(format!("magnet:?xt=urn:btih:{hex}")), AddStep::AlreadyInList(_)));
    assert!(matches!(core.add_url("ftp://x"), AddStep::Error(_, d) if d == "ftp://x"));
    assert!(matches!(core.add_url("  "), AddStep::Nothing));
}

#[test]
fn seeding_only_hours_put_every_torrent_in_upload_mode() {
    let mut s = Setup::new();
    s.options.gui.schedule.enabled = true;
    s.options.gui.schedule.table = [Mode::SeedOnly; crate::scheduler::CELLS];
    s.mock.initial_torrents = 3;
    let mut core = s.start("seed-only");
    pump(&mut core, "the torrents", |c| c.list().len() == 3);
    let ev = core.tick(Instant::now());
    assert_eq!(core.scheduler.mode(), Mode::SeedOnly);
    assert!(ev.status);
    assert!(core.status_bar().net.ends_with(&format!(" | {}", tr!(SchedStatusSeedingOnly))));
    assert!(logged(&core, &tr!(LogScheduler, mode = Mode::SeedOnly.label())));
    let upload_mode = |c: &Core| {
        c.list().len() == 4
            && c.list().ids().all(|id| c.list().get(id).unwrap().flags.contains(TorrentFlags::UPLOAD_MODE))
    };
    // a torrent added now joins in upload mode too
    core.prefs_mut().set_bool("ui.show_add_dialog", false);
    core.add_file(torrent("late"));
    pump(&mut core, "upload mode everywhere", upload_mode);
}

#[test]
fn a_seed_past_its_goal_is_stopped() {
    let mut s = Setup::new();
    s.options.gui.seed_ratio = 150;
    let mut core = s.start("goal");
    let mut st = status(7, 7, "seed", true);
    st.all_time_upload = 300;
    st.all_time_download = 100;
    st.total_wanted_done = 100;
    core.handle_alerts(vec![Alert::StateUpdate(vec![st.clone()])]);
    assert!(logged(&core, &tr!(LogSeedingGoal, name = "seed", what = tr!(SeedingGoalStopped))));
    // its own goal, higher, wins
    let mut other = status(8, 8, "own", true);
    other.all_time_upload = 300;
    other.all_time_download = 100;
    let key = hash(8).key_hex();
    SeedGoal::store_override(core.prefs_mut(), &key, Some(SeedGoal { ratio_pct: 500, minutes: 0 }));
    core.handle_alerts(vec![Alert::StateUpdate(vec![other])]);
    assert!(!logged(&core, &tr!(LogSeedingGoal, name = "own", what = tr!(SeedingGoalStopped))));
}

#[test]
fn applied_options_reach_every_torrent_the_ip_filter_and_the_scheduler() {
    let mut s = Setup::new();
    s.mock.initial_torrents = 2;
    let mut core = s.start("options");
    pump(&mut core, "the torrents", |c| c.list().len() == 2);
    let filter = core.dirs().settings.join("ipfilter.dat");
    std::fs::write(&filter, "1.2.3.0 - 1.2.3.255 , 000 , test\n").unwrap();
    // PEX off, the IP filter on, the scheduler's Off hours
    let mut o = core.options();
    assert!(o.gui.pex);
    o.gui.pex = false;
    o.gui.ip_filter = true;
    o.gui.schedule.enabled = true;
    o.gui.schedule.table = [Mode::Off; crate::scheduler::CELLS];
    o.gui.alternate_rows = !o.gui.alternate_rows;
    let ev = core.apply_options(&o);
    assert_eq!(ev.alternate_rows, Some(o.gui.alternate_rows));
    assert!(logged(&core, &tr!(LogIpFilterRules, count = 1, path = filter.display())));
    assert_eq!(core.scheduler.mode(), Mode::Off);
    assert!(core.status_bar().net.ends_with(tr!(SchedStatusOff)));
    assert_eq!(core.options(), o);
    pump(&mut core, "PEX off everywhere", |c| {
        c.list().ids().all(|id| c.list().get(id).unwrap().flags.contains(TorrentFlags::DISABLE_PEX))
    });
    // and back
    o.gui.pex = true;
    o.gui.ip_filter = false;
    o.gui.schedule.enabled = false;
    core.apply_options(&o);
    assert!(logged(&core, tr!(LogIpFilterOff)));
    assert_eq!(core.scheduler.mode(), Mode::Full);
    assert_eq!(core.status_bar().net, tr!(NetNoIncoming));
    pump(&mut core, "PEX on everywhere", |c| {
        c.list().ids().all(|id| !c.list().get(id).unwrap().flags.contains(TorrentFlags::DISABLE_PEX))
    });
    // nothing changed: the filter is not loaded again
    let lines = core.detail().log.len();
    core.apply_options(&o);
    assert_eq!(core.detail().log.len(), lines);
}

#[test]
fn status_bar_title_and_tray_texts() {
    let mut core = Setup::new().start("status");
    let stats = SessionStats {
        dht_nodes: 42,
        download_rate: 2048,
        upload_rate: 1024,
        has_incoming_connections: true,
        ..SessionStats::default()
    };
    core.handle_alerts(vec![Alert::SessionStats(stats)]);
    let ev = core.events();
    assert!(ev.status);
    let bar = core.status_bar();
    assert_eq!(bar.net, tr!(NetOk));
    assert_eq!(bar.net_icon, crate::icons::network_icon(true));
    assert_eq!(bar.dht, tr!(BarDht, count = 42));
    assert_eq!([bar.down, bar.up], crate::status::bar_rates(&stats));
    assert!(!bar.port_taken && bar.update.is_none());
    assert_eq!(core.title(), profile::name_build());
    assert_eq!(
        core.tray_tip(),
        tr!(
            TrayTip,
            version = profile::version(),
            down = fmt::speed_always(2048),
            up = fmt::speed_always(1024)
        )
    );
    // "Show current speed in the title bar"
    let mut o = core.options();
    o.gui.title_speed = true;
    let ev = core.apply_options(&o);
    assert!(ev.title);
    assert_eq!(core.title(), fmt::title_speed(2048, 1024, &profile::name_build()));
    // the Logger's time stamp
    assert_eq!(hms(NOON + 3723, 3600), "[14:02:03]");
    assert!(core.detail().log.iter().all(|l| l.starts_with("[12:00:00] ")));
}

#[test]
fn a_remote_window_names_its_server_and_says_when_it_is_lost() {
    let mut s = Setup::new();
    s.remote = Some(Remote { label: "nas:8080".into() });
    let mut core = s.start("remote");
    assert_eq!(core.title(), tr!(TitleRemote, version = profile::version(), label = "nas:8080"));
    assert!(core.menu_state(&[TorrentId::from_raw(1)]).is_some_and(|m| !m.can_open_folder));
    core.handle_alerts(vec![Alert::RemoteDisconnected { message: "gone".into() }]);
    let ev = core.events();
    assert!(ev.remote_lost && ev.title);
    assert_eq!(ev.errors, [(tr!(ErrRemoteLost, label = "nas:8080", message = "gone"), String::new())]);
    assert_eq!(core.title(), tr!(TitleDisconnected, version = profile::version(), label = "nas:8080"));
}

#[test]
fn menu_state_and_remove_question_follow_the_selection() {
    let mut core = Setup::new().start("menu");
    let mut a = status(1, 1, "a", true);
    a.download_limit = 50 * 1024;
    a.upload_limit = 1000;
    let b = status(2, 2, "b", false);
    core.handle_alerts(vec![Alert::StateUpdate(vec![a, b])]);
    let (a, b) = (TorrentId::from_raw(1), TorrentId::from_raw(2));
    assert!(core.menu_state(&[]).is_none());
    let m = core.menu_state(&[a]).unwrap();
    assert!(m.single_done && m.can_super_seed && !m.super_seeding && m.can_open_folder);
    assert_eq!((m.download_kib, m.upload_kib), (50, u32::MAX));
    assert_eq!(m.label.as_deref(), Some(""));
    assert!(!m.can_remove_label);
    core.set_label(&[a], "x");
    let m = core.menu_state(&[a, b]).unwrap();
    assert!(!m.single_done && !m.can_super_seed && m.label.is_none() && m.can_remove_label);
    assert_eq!(m.labels, ["x"]);
    // "Confirm when deleting torrents" is on by default; with the data it is always asked
    assert_eq!(core.remove_question(&[a], false).unwrap().0, tr!(AskRemove, name = "a"));
    assert_eq!(core.remove_question(&[a, b], true).unwrap().0, tr!(AskRemoveDataMany, count = 2));
    core.prefs_mut().set_bool("ui.confirm_delete", false);
    assert!(core.remove_question(&[a], false).is_none());
    assert!(core.remove_question(&[a], true).is_some());
}

#[test]
fn the_detail_tabs_show_the_first_selected_torrent() {
    let mut s = Setup::new();
    s.prefs.set_bool(&format!("torrent.{}.paused", hash(2).key_hex()), true);
    let mut core = s.start("detail-torrent");
    core.handle_alerts(vec![Alert::StateUpdate(vec![status(1, 1, "a", false), status(2, 2, "b", false)])]);
    let (a, b) = (TorrentId::from_raw(1), TorrentId::from_raw(2));
    assert!(core.detail_torrent().is_none());
    core.select(&[b, a]);
    let (st, paused) = core.detail_torrent().unwrap();
    assert_eq!((st.name.as_str(), paused), ("b", true));
    core.select(&[a]);
    let (st, paused) = core.detail_torrent().unwrap();
    assert_eq!((st.name.as_str(), paused), ("a", false));
    core.show_tab(Tab::Files);
    assert_eq!(core.tab(), Tab::Files);
    assert!(core.events().detail);
    core.select(&[]);
    assert!(core.detail_torrent().is_none());
}

/// A `.torrent` named `name` holding `files` (path parts, size), one 16 KiB piece.
fn multi_torrent(name: &str, files: &[(&[&str], u64)]) -> Vec<u8> {
    let bstr = |s: &str| format!("{}:{s}", s.len());
    let mut list = String::new();
    for (parts, size) in files {
        let path: String = parts.iter().map(|p| bstr(p)).collect();
        list.push_str(&format!("d6:lengthi{size}e4:pathl{path}ee"));
    }
    let mut b =
        format!("d4:infod5:filesl{list}e4:name{}12:piece lengthi16384e6:pieces20:", bstr(name)).into_bytes();
    b.extend([b'a'; 20]);
    b.extend(b"ee");
    b
}

/// The tree's node named `name`, searched from the top.
fn node_named(core: &Core, name: &str) -> usize {
    let tree = core.detail().file_tree().expect("the tree");
    let mut todo = tree.children(None);
    while let Some(n) = todo.pop() {
        if tree.node(n).unwrap().name == name {
            return n;
        }
        todo.extend(tree.children(Some(n)));
    }
    panic!("no node {name}")
}

#[test]
fn the_files_tree_acts_on_nodes() {
    use rookparcel_core::DownloadPriority;
    let mut core = Setup::new().start("files-tree");
    let save = temp("files-tree-save");
    let files: [(&[&str], u64); 3] =
        [(&["Season 1", "e01.mkv"], 1000), (&["Season 1", "e02.mkv"], 3000), (&["info.nfo"], 5)];
    let AddStep::Dialog(r) = core.add_file(multi_torrent("Show", &files)) else { panic!("no dialog") };
    core.finish_add(r, Some(AddChoice { save_path: save.to_string_lossy().into_owned(), ..choice() }));
    pump(&mut core, "the torrent", |c| c.list().len() == 1);
    let id = core.list().ids().next().unwrap();
    core.select(&[id]);
    core.show_tab(Tab::Files);
    pump(&mut core, "its files", |c| c.detail().file_tree().is_some());
    let prio =
        |c: &Core| -> Vec<u8> { c.detail().files.as_ref().unwrap().iter().map(|f| f.priority.0).collect() };

    // a folder's path, a file's; a node that is not there has none
    let season = node_named(&core, "Season 1");
    let e02 = node_named(&core, "e02.mkv");
    let show = save.join("Show");
    assert_eq!(core.file_node_path(season), Some((save.clone(), show.join("Season 1"))));
    assert_eq!(core.file_node_path(e02), Some((save.clone(), show.join("Season 1").join("e02.mkv"))));
    assert_eq!(core.file_node_path(99), None);

    // the folder's files take the priority
    let under = core.detail().file_tree().unwrap().files_under_nodes(&[season]);
    assert_eq!(under, [0, 1]);
    core.set_file_priority_files(&under, Some(DownloadPriority::DONT_DOWNLOAD));
    pump(&mut core, "the new priorities", |c| prio(c) == [0, 0, 4]);
    // the rows' version maps rows the same way: row 0 is the folder
    core.set_file_priority(&[0], Some(DownloadPriority::NORMAL));
    pump(&mut core, "the folder back to Normal", |c| prio(c) == [4, 4, 4]);
    // Prioritize by File Order takes every file while the filter hides some
    assert!(core.set_file_filter("nfo"));
    core.set_file_priority_files(&[2], None);
    pump(&mut core, "by file order", |c| prio(c) == [7, 5, 3]);
    assert!(core.set_file_filter(""));

    // a folder is never relocated; nor is anything in the plain list by node
    core.relocate_file_node(season, &save.join("elsewhere"));
    assert!(core.set_files_tree(false));
    assert_eq!(core.file_node_path(e02), None);
}
