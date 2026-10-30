//! The commands (docs/WEBUI.md "Commands"): one implementation behind the native `POST
//! /pa/...` endpoints and the compat shim's `torrents/*` handlers. Everything here takes the
//! [`Shared`] cache and the session handle; the two front doors (`native` in this file and
//! `api.rs` for the shim) only parse their own request shapes and map the results to their own
//! status codes.
//!
//! Start / Force Start / Pause / Stop are `frontend::actions` over libtorrent's flags; the
//! user's Pause (as opposed to Stop) is remembered per torrent in [`State::user_paused`], so
//! the cells and the status class show "Paused" as the Win32 list does. Adding goes through
//! `frontend::add::AddChoice`, the Add New Torrent dialog's model, so the Web UI and the Win32
//! dialog add the same way.

use std::collections::{BTreeMap, HashMap, HashSet};

use patorrent_core::fetch::FetchProxy;
use patorrent_core::metainfo::{TorrentInfo, parse_magnet_uri};
use patorrent_core::{
    AddTorrentParams, AnnounceEntry, DownloadPriority, MoveFlags, QueueMove, RemoveFlags, TorrentFlags,
    TorrentId, TorrentSource,
};
use patorrent_frontend::actions;
use patorrent_frontend::add::{self, AddChoice, AwaitingMetadata};
use patorrent_frontend::fmt;
use patorrent_frontend::properties::{self, Changes, Field, Fields, Properties, TorrentProps, Tri};
use patorrent_frontend::scheduler::Mode;
use patorrent_frontend::seeding::SeedGoal;
use patorrent_frontend::status::Category;

use super::api::Request;
use super::http::{
    Head, Part, Response, multipart_boundary, parse_header_params, parse_multipart, parse_urlencoded,
};
use super::json::{self, Json};
use super::native::category_from_id;
use super::torrent::hash_of;
use super::{Entry, Meta, Reply, ReplyKind, Shared, State, fetch, prefs};

/// The header every native command must carry (a browser cannot send a custom header
/// cross-site without a CORS preflight, which the server never grants): `X-Requested-With:
/// PaTorrent`. The `Origin` rule of the shim applies too.
pub const CSRF_HEADER: &str = "x-requested-with";
pub const CSRF_VALUE: &str = "PaTorrent";

/// A refused command: the HTTP status and the message for `{"error":"..."}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdError {
    pub status: u16,
    pub msg: String,
}

fn err(status: u16, msg: impl Into<String>) -> CmdError {
    CmdError { status, msg: msg.into() }
}

fn not_found() -> CmdError {
    err(404, "no such torrent")
}

/// The view filter of a client, for `"all": true`: the same rules as the `view` subscription
/// (`native.rs`): a category or a label, and a case-insensitive name search.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewFilter {
    pub category: Option<Category>,
    /// `Some("")` means "no label".
    pub label: Option<String>,
    /// Lower-case; empty for none.
    pub search: String,
}

impl ViewFilter {
    pub fn matches(&self, st: &State, id: TorrentId) -> bool {
        let Some(e) = st.torrents.get(&id) else { return false };
        if let Some(c) = self.category {
            if !c.contains(&e.status) {
                return false;
            }
        }
        if let Some(l) = &self.label {
            if st.torrent_categories.get(&e.hash).map_or("", String::as_str) != l {
                return false;
            }
        }
        self.search.is_empty() || e.status.name.to_lowercase().contains(&self.search)
    }
}

/// Which torrents a command acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Info-hashes in hex; unknown ones are skipped.
    Ids(Vec<String>),
    /// Every torrent the filter matches (every torrent, without one).
    All(ViewFilter),
}

impl Selection {
    /// `{"ids":[...]}` or `{"all":true,"filter":{"category":...,"label":...,"search":...}}`.
    pub fn parse(body: &Json) -> Result<Selection, &'static str> {
        if body.get("all").and_then(Json::as_bool) == Some(true) {
            let f = body.get("filter");
            let label = f.and_then(|f| f.get("label")).and_then(Json::as_str).map(str::to_owned);
            let category = match f.and_then(|f| f.get("category")).and_then(Json::as_str) {
                Some(id) if label.is_none() => Some(category_from_id(id).ok_or("unknown category")?),
                _ => None,
            };
            let search =
                f.and_then(|f| f.get("search")).and_then(Json::as_str).unwrap_or("").trim().to_lowercase();
            return Ok(Selection::All(ViewFilter { category, label, search }));
        }
        match body.get("ids") {
            Some(Json::Arr(v)) => {
                let ids = v
                    .iter()
                    .map(|x| x.as_str().map(|s| s.trim().to_ascii_lowercase()).ok_or("ids must be strings"))
                    .collect::<Result<Vec<String>, _>>()?;
                if ids.is_empty() {
                    return Err("ids is empty");
                }
                Ok(Selection::Ids(ids))
            }
            _ => Err("missing ids"),
        }
    }

    /// The torrent ids the selection names right now, in no particular order.
    pub fn resolve(&self, st: &State) -> Vec<TorrentId> {
        match self {
            Selection::Ids(ids) => ids.iter().filter_map(|h| st.by_hash.get(h).copied()).collect(),
            Selection::All(f) => st.torrents.keys().copied().filter(|&id| f.matches(st, id)).collect(),
        }
    }
}

/// The one torrent an `{"id":"..."}` body names.
pub fn one(st: &State, body: &Json) -> Result<TorrentId, CmdError> {
    let hash = body.get("id").and_then(Json::as_str).ok_or_else(|| err(400, "missing id"))?;
    st.by_hash.get(&hash.trim().to_ascii_lowercase()).copied().ok_or_else(not_found)
}

// ---- the torrent commands ----------------------------------------------------------------

/// Start: back into the queue (auto-managed) and resumed.
pub fn start(shared: &Shared, ids: &[TorrentId]) {
    set_paused_mark(shared, ids, false);
    for &id in ids {
        actions::start(&shared.handle.torrent(id));
    }
}

/// Force Start: out of the queue and running.
pub fn force_start(shared: &Shared, ids: &[TorrentId]) {
    set_paused_mark(shared, ids, false);
    for &id in ids {
        actions::force_start(&shared.handle.torrent(id));
    }
}

/// The shim's `setForceStart` with `value=false`: back under the queue's control, without a
/// resume (qBittorrent's semantics).
pub fn unforce(shared: &Shared, ids: &[TorrentId]) {
    for &id in ids {
        shared.handle.torrent(id).set_flags(TorrentFlags::AUTO_MANAGED);
    }
}

/// Pause: stopped in the engine, remembered as paused.
pub fn pause(shared: &Shared, ids: &[TorrentId]) {
    set_paused_mark(shared, ids, true);
    for &id in ids {
        actions::stop(&shared.handle.torrent(id));
    }
}

/// Stop.
pub fn stop(shared: &Shared, ids: &[TorrentId]) {
    set_paused_mark(shared, ids, false);
    for &id in ids {
        actions::stop(&shared.handle.torrent(id));
    }
}

/// Records (or clears) the user's Pause for the torrents and bumps their versions, so the
/// next tick carries the new status cell.
fn set_paused_mark(shared: &Shared, ids: &[TorrentId], paused: bool) {
    let mut st = shared.lock();
    let mut changed = false;
    for id in ids {
        let Some(hash) = st.torrents.get(id).map(|e| e.hash.clone()) else { continue };
        changed |= st.set_user_paused(&hash, paused);
    }
    if changed {
        shared.save(&mut st);
    }
}

pub fn recheck(shared: &Shared, ids: &[TorrentId]) {
    for &id in ids {
        shared.handle.torrent(id).force_recheck();
    }
}

pub fn reannounce(shared: &Shared, ids: &[TorrentId]) {
    for &id in ids {
        shared.handle.torrent(id).force_reannounce();
    }
}

/// Remove, or Remove And > Delete Data.
pub fn remove(shared: &Shared, ids: &[TorrentId], data: bool) {
    let mut flags = RemoveFlags::DELETE_PARTFILE;
    if data {
        flags |= RemoveFlags::DELETE_FILES;
    }
    for &id in ids {
        shared.handle.remove_torrent(id, flags);
    }
}

pub fn queue_move_from(s: &str) -> Option<QueueMove> {
    match s {
        "up" => Some(QueueMove::Up),
        "down" => Some(QueueMove::Down),
        "top" => Some(QueueMove::Top),
        "bottom" => Some(QueueMove::Bottom),
        _ => None,
    }
}

/// The order a queue move must be applied to several torrents so they keep their relative
/// order: Up and Bottom from the lowest queue position, Down and Top from the highest (each
/// move is one engine call, and the last one moved to the top ends up first). Torrents
/// outside the queue (stopped, seeding without a position) are left out.
pub fn queue_order(st: &State, ids: &[TorrentId], m: QueueMove) -> Vec<TorrentId> {
    let mut v: Vec<(u32, TorrentId)> = ids
        .iter()
        .filter_map(|&id| st.torrents.get(&id).and_then(|e| e.status.queue_position).map(|p| (p, id)))
        .collect();
    v.sort_unstable();
    if matches!(m, QueueMove::Down | QueueMove::Top) {
        v.reverse();
    }
    v.into_iter().map(|(_, id)| id).collect()
}

pub fn queue(shared: &Shared, ids: &[TorrentId], m: QueueMove) {
    let ordered = queue_order(&shared.lock(), ids, m);
    for id in ordered {
        shared.handle.torrent(id).queue_position(m);
    }
    shared.handle.post_torrent_updates();
}

/// Per-torrent limits in bytes per second, 0 for unlimited; `None` leaves one alone.
pub fn limits(shared: &Shared, ids: &[TorrentId], down: Option<u32>, up: Option<u32>) {
    for &id in ids {
        let t = shared.handle.torrent(id);
        if let Some(d) = down {
            t.set_download_limit(d);
        }
        if let Some(u) = up {
            t.set_upload_limit(u);
        }
    }
}

/// A label a torrent can carry: no leading, trailing or doubled `/`, no `\`, no control
/// characters (the shim's category rule).
pub fn valid_label(name: &str) -> bool {
    !name.starts_with('/')
        && !name.ends_with('/')
        && !name.contains("//")
        && !name.contains('\\')
        && name.chars().all(|c| !c.is_control())
}

/// Sets the torrents' label (empty removes it), creating the label when it is new (the Win32
/// "New Label..."); the shim's `setCategory` checks for an unknown one before calling.
pub fn label(shared: &Shared, ids: &[TorrentId], label: &str) -> Result<(), CmdError> {
    let label = label.trim();
    if !valid_label(label) {
        return Err(err(400, "invalid label"));
    }
    let mut st = shared.lock();
    if !label.is_empty() && !st.categories.contains_key(label) {
        st.categories.insert(label.to_owned(), String::new());
        st.categories_version += 1;
    }
    for id in ids {
        let Some(hash) = st.torrents.get(id).map(|e| e.hash.clone()) else { continue };
        st.set_label(&hash, label);
    }
    shared.save(&mut st);
    Ok(())
}

/// Set Download Location: moves the torrents' files (`move_storage`), skipping those already
/// there, as the Win32 command does.
pub fn location(shared: &Shared, ids: &[TorrentId], dir: &str) -> Result<(), CmdError> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err(err(400, "missing path"));
    }
    shared.path_policy.check(dir).map_err(|why| err(400, why))?;
    let moving: Vec<TorrentId> = {
        let st = shared.lock();
        ids.iter()
            .copied()
            .filter(|id| st.torrents.get(id).is_some_and(|e| !same_dir(&e.status.save_path, dir)))
            .collect()
    };
    for id in moving {
        shared.handle.torrent(id).move_storage(dir, MoveFlags::AlwaysReplaceFiles);
    }
    shared.handle.post_torrent_updates();
    Ok(())
}

fn same_dir(a: &str, b: &str) -> bool {
    let t = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_lowercase();
    t(a) == t(b)
}

/// Initial Seeding on or off.
pub fn super_seed(shared: &Shared, ids: &[TorrentId], on: bool) {
    for &id in ids {
        let t = shared.handle.torrent(id);
        if on {
            t.set_flags(TorrentFlags::SUPER_SEEDING);
        } else {
            t.unset_flags(TorrentFlags::SUPER_SEEDING);
        }
    }
}

// --- Torrent Properties (`frontend::properties`) ----------------------------------------------

/// Each torrent's tracker list, asked of the engine (outside the lock); a torrent that does not
/// answer in time is left out.
fn tracker_lists(shared: &Shared, ids: &[TorrentId]) -> HashMap<TorrentId, Vec<AnnounceEntry>> {
    shared
        .query_many(ids, ReplyKind::Trackers, |h, id| h.torrent(id).post_trackers())
        .into_iter()
        .filter_map(|(id, r)| match r {
            Reply::Trackers(list) => Some((id, list)),
            _ => None,
        })
        .collect()
}

/// The Properties dialog's model for `ids`, as the Win32 dialog loads it.
pub fn properties(shared: &Shared, ids: &[TorrentId]) -> Result<Properties, CmdError> {
    let trackers = tracker_lists(shared, ids);
    let st = shared.lock();
    let items: Vec<TorrentProps> = ids
        .iter()
        .filter_map(|id| {
            let e = st.torrents.get(id)?;
            Some(TorrentProps::new(&e.status, trackers.get(id).map(Vec::as_slice), &st.options))
        })
        .collect();
    if items.is_empty() {
        return Err(not_found());
    }
    Ok(Properties::load(&items, st.prefs.gui().property_defaults()))
}

fn tri_name(t: Tri) -> &'static str {
    match t {
        Tri::Off => "off",
        Tri::On => "on",
        Tri::Mixed => "mixed",
    }
}

/// `GET /pa/torrents/properties?ids=<hash>,<hash>`: the fields (text as the dialog shows it,
/// check boxes as `"on"`, `"off"` or `"mixed"`), how many torrents, and whether Initial
/// Seeding can be set.
pub fn properties_json(p: &Properties) -> Json {
    let f = &p.fields;
    let fields = Json::obj()
        .with("trackers", f.trackers.as_str())
        .with("upload_kib", f.upload_kib.as_str())
        .with("download_kib", f.download_kib.as_str())
        .with("override_slots", tri_name(f.override_slots))
        .with("upload_slots", f.upload_slots.as_str())
        .with("override_goal", tri_name(f.override_goal))
        .with("goal_ratio", f.goal_ratio.as_str())
        .with("goal_minutes", f.goal_minutes.as_str())
        .with("initial_seeding", tri_name(f.initial_seeding))
        .with("dht", tri_name(f.dht))
        .with("pex", tri_name(f.pex));
    Json::obj().with("count", p.count).with("can_initial_seed", p.can_initial_seed).with("fields", fields)
}

/// The `GET`'s selection: `ids=<hash>,<hash>`, or `all=1` with the view's `category`, `label`
/// and `search`, as a command's `{"all":true,"filter":{...}}` (the page's Ctrl+A).
fn query_selection(q: &[(String, String)]) -> Result<Selection, CmdError> {
    let get = |k: &str| q.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    if matches!(get("all"), Some("1" | "true")) {
        let mut f = Json::obj();
        for k in ["category", "label", "search"] {
            if let Some(v) = get(k) {
                f.set(k, v);
            }
        }
        return Selection::parse(&Json::obj().with("all", true).with("filter", f)).map_err(|e| err(400, e));
    }
    let hashes = get("ids").unwrap_or("").split(',');
    Ok(Selection::Ids(hashes.map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()).collect()))
}

/// The `GET`'s answer: [`properties_json`], plus the torrent's `name` when there is one (the
/// page's dialog title) and whether every torrent is `finished` (a double-click opens the
/// dialog only on a torrent that is not, as in the Win32 window).
fn properties_reply(shared: &Shared, sel: &Selection) -> Result<Json, CmdError> {
    let ids = sel.resolve(&shared.lock());
    let mut out = properties_json(&properties(shared, &ids)?);
    let st = shared.lock();
    let entries: Vec<&Entry> = ids.iter().filter_map(|id| st.torrents.get(id)).collect();
    if let [one] = entries[..] {
        out.set("name", one.status.name.as_str());
    }
    out.set("finished", !entries.is_empty() && entries.iter().all(|e| e.status.is_finished));
    Ok(out)
}

/// Sets the fields a partial `fields` object names (same names and values as the `GET`).
fn overlay(f: &mut Fields, body: &Json) -> Result<(), CmdError> {
    for (k, v) in body.entries() {
        let text = || v.as_str().map(str::to_owned).ok_or_else(|| err(400, format!("{k} must be a string")));
        let tri = || match v.as_str() {
            Some("on") => Ok(Tri::On),
            Some("off") => Ok(Tri::Off),
            Some("mixed") => Ok(Tri::Mixed),
            _ => match v.as_bool() {
                Some(b) => Ok(if b { Tri::On } else { Tri::Off }),
                None => Err(err(400, format!("{k} must be on, off or mixed"))),
            },
        };
        match k.as_str() {
            "trackers" => f.trackers = text()?,
            "upload_kib" => f.upload_kib = text()?,
            "download_kib" => f.download_kib = text()?,
            "upload_slots" => f.upload_slots = text()?,
            "goal_ratio" => f.goal_ratio = text()?,
            "goal_minutes" => f.goal_minutes = text()?,
            "override_slots" => f.override_slots = tri()?,
            "override_goal" => f.override_goal = tri()?,
            "initial_seeding" => f.initial_seeding = tri()?,
            "dht" => f.dht = tri()?,
            "pex" => f.pex = tri()?,
            _ => return Err(err(400, format!("unknown field {k}"))),
        }
    }
    Ok(())
}

fn field_name(f: Field) -> &'static str {
    match f {
        Field::Trackers => "trackers",
        Field::UploadRate => "upload_kib",
        Field::DownloadRate => "download_kib",
        Field::UploadSlots => "upload_slots",
        Field::GoalRatio => "goal_ratio",
        Field::GoalMinutes => "goal_minutes",
    }
}

/// `POST /pa/torrents/properties`: the selection plus `fields`; the fields left out stay as
/// loaded, and so what they hold stays on every torrent. 400 names the first bad field.
pub fn set_properties(shared: &Shared, ids: &[TorrentId], fields: &Json) -> Result<(), CmdError> {
    let mut p = properties(shared, ids)?;
    overlay(&mut p.fields, fields)?;
    let changes = p.changes().map_err(|e| err(400, format!("{}: {}", field_name(e.field), e.message)))?;
    apply_properties(shared, ids, &changes);
    Ok(())
}

/// The engine calls, and the seeding goal override in the stored preferences.
pub fn apply_properties(shared: &Shared, ids: &[TorrentId], changes: &Changes) {
    if changes.is_empty() {
        return;
    }
    let mut st = shared.lock();
    let defaults = st.prefs.gui().property_defaults();
    for id in ids {
        let Some(key) = st.torrents.get(id).map(|e| e.status.info_hashes.key_hex()) else { continue };
        changes.apply(&shared.handle.torrent(*id));
        changes.store(&mut st.options, &key, &defaults);
    }
    if changes.seed_goal.is_some() {
        shared.save(&mut st);
    }
}

/// The shim's `editTracker`: one URL replaced in place. 400 for a URL that is not a tracker,
/// 409 when the old one is not in the list or the new one already is.
pub fn edit_tracker(shared: &Shared, id: TorrentId, orig: &str, new: &str) -> Result<(), CmdError> {
    let new = new.trim();
    properties::parse_trackers(new)
        .ok()
        .filter(|t| t.len() == 1)
        .ok_or_else(|| err(400, "newUrl is not a valid URL"))?;
    let list = tracker_lists(shared, &[id]).remove(&id).ok_or_else(not_found)?;
    if list.iter().any(|t| t.url == new) {
        return Err(err(409, "newUrl already exists for the torrent"));
    }
    let Some(i) = list.iter().position(|t| t.url == orig.trim()) else {
        return Err(err(409, "origUrl was not found"));
    };
    let mut entries: Vec<AnnounceEntry> =
        list.iter().map(|t| AnnounceEntry::new(t.url.clone(), t.tier)).collect();
    entries[i].url = new.to_owned();
    shared.handle.torrent(id).replace_trackers(entries);
    Ok(())
}

/// The shim's `addTrackers`: each new URL in a tier of its own after the existing ones (lines
/// that are not tracker URLs are skipped, as the shim reports nothing per URL).
pub fn add_trackers(shared: &Shared, ids: &[TorrentId], urls: &str) {
    let lists = tracker_lists(shared, ids);
    let valid = |u: &&str| properties::parse_trackers(u).is_ok_and(|v| !v.is_empty());
    for &id in ids {
        let first = lists.get(&id).and_then(|l| l.iter().map(|t| t.tier).max()).map_or(0, |t| t + 1);
        let t = shared.handle.torrent(id);
        for (tier, url) in (first..).zip(urls.lines().map(str::trim).filter(valid)) {
            t.add_tracker(AnnounceEntry::new(url, tier));
        }
    }
}

/// The shim's `removeTrackers`: `urls` `|`-separated. 409 when none of them was there.
pub fn remove_trackers(shared: &Shared, ids: &[TorrentId], urls: &str) -> Result<(), CmdError> {
    let gone: HashSet<&str> = urls.split('|').map(str::trim).filter(|u| !u.is_empty()).collect();
    let mut removed = false;
    for (id, list) in tracker_lists(shared, ids) {
        if !list.iter().any(|t| gone.contains(t.url.as_str())) {
            continue;
        }
        removed = true;
        let keep = list
            .iter()
            .filter(|t| !gone.contains(t.url.as_str()))
            .map(|t| AnnounceEntry::new(t.url.clone(), t.tier))
            .collect();
        shared.handle.torrent(id).replace_trackers(keep);
    }
    if removed { Ok(()) } else { Err(err(409, "All urls were not found")) }
}

/// The shim's share limits as a torrent's own seeding goal: `ratio` (-2 the default, -1 none)
/// and `minutes` (the same); both -2 is no goal of its own.
pub fn share_goal(default: SeedGoal, ratio: f64, minutes: i64) -> Option<SeedGoal> {
    (ratio != -2.0 || minutes != -2).then(|| SeedGoal {
        ratio_pct: if ratio == -2.0 {
            default.ratio_pct
        } else {
            (ratio.max(0.0) * 100.0).round().min(1e6) as u32
        },
        minutes: if minutes == -2 { default.minutes } else { minutes.clamp(0, 1_000_000) as u32 },
    })
}

/// The shim's `setShareLimits` on the seeding goal ([`share_goal`]).
pub fn share_limits(shared: &Shared, ids: &[TorrentId], ratio: f64, minutes: i64) {
    let mut st = shared.lock();
    let goal = share_goal(st.prefs.seeding_goal.goal, ratio, minutes);
    for id in ids {
        let Some((key, hash)) = st.torrents.get(id).map(|e| (e.status.info_hashes.key_hex(), e.hash.clone()))
        else {
            continue;
        };
        SeedGoal::store_override(&mut st.options, &key, goal);
        // `ratio_limit` and `seeding_time_limit` changed: `sync/maindata` sends them again
        st.touch(&hash);
    }
    shared.save(&mut st);
}

/// The engine's file list, or the shim's 409 when the metadata is not there yet.
fn file_list(
    shared: &Shared,
    id: TorrentId,
) -> Result<std::sync::Arc<[patorrent_core::FileEntry]>, CmdError> {
    match shared.query(id, ReplyKind::Files, |h| h.torrent(id).post_file_list()) {
        Some(Reply::Files(f)) if !f.is_empty() => Ok(f),
        _ => Err(err(409, "torrent metadata hasn't downloaded yet")),
    }
}

/// File priorities 0..7 per file index (`frontend::detail::PRIORITIES`: 7 high, 4 normal, 1
/// low, 0 don't download).
pub fn files(shared: &Shared, id: TorrentId, priorities: &[(u32, DownloadPriority)]) -> Result<(), CmdError> {
    if priorities.iter().any(|(_, p)| p.0 > 7) {
        return Err(err(400, "priority must be 0..7"));
    }
    let list = file_list(shared, id)?;
    if priorities.iter().any(|&(i, _)| i as usize >= list.len()) {
        return Err(err(409, "file id not found"));
    }
    let t = shared.handle.torrent(id);
    for &(i, p) in priorities {
        t.set_file_priority(i, p);
    }
    t.post_file_list();
    Ok(())
}

/// A file path in the API's form: `/` separators, no leading or trailing ones.
pub fn api_path(p: &str) -> String {
    p.replace('\\', "/").trim_matches('/').to_owned()
}

/// A new name: relative, no empty, `.` or `..` parts, no characters Windows forbids.
pub fn valid_new_path(p: &str) -> bool {
    !p.is_empty()
        && !p.contains(':')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
        && !p.chars().any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*') || c.is_control())
}

/// Renames one file (`folder` false), every file under a folder (true), or whichever `old`
/// names (`None`: a file first, else a folder). Paths as the Files tab shows them, torrent
/// folder included. The rename runs in the background.
pub fn rename(
    shared: &Shared,
    id: TorrentId,
    old: &str,
    new: &str,
    folder: Option<bool>,
) -> Result<(), CmdError> {
    let absolute = new.starts_with(['/', '\\']);
    let (old, new) = (api_path(old), api_path(new));
    if absolute || !valid_new_path(&new) {
        return Err(err(409, "invalid new path"));
    }
    let files = file_list(shared, id)?;
    let paths: Vec<String> = files.iter().map(|f| api_path(&f.path)).collect();
    let folder = folder.unwrap_or_else(|| !paths.contains(&old));
    let mut todo = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let renamed = if !folder {
            (*path == old).then(|| new.clone())
        } else {
            path.strip_prefix(&old)
                .and_then(|rest| rest.strip_prefix('/'))
                .map(|rest| format!("{new}/{rest}"))
        };
        if let Some(name) = renamed {
            todo.push((i as u32, name));
        }
    }
    if todo.is_empty() {
        return Err(err(409, if folder { "no such folder" } else { "no such file" }));
    }
    // The new names must not collide with files that keep theirs.
    let moving: Vec<usize> = todo.iter().map(|&(i, _)| i as usize).collect();
    let taken = |n: &str| {
        paths.iter().enumerate().any(|(i, p)| {
            !moving.contains(&i)
                && (p == n || p.starts_with(&format!("{n}/")) || n.starts_with(&format!("{p}/")))
        })
    };
    if todo.iter().any(|(_, n)| taken(n)) {
        return Err(err(409, "new path already in use"));
    }
    let h = shared.handle.torrent(id);
    for (i, name) in todo {
        h.rename_file(i, name);
    }
    Ok(())
}

// ---- adding ------------------------------------------------------------------------------

/// What to add.
pub enum Source {
    /// A `.torrent` file's bytes and what they parsed to.
    Torrent(Vec<u8>, Box<TorrentInfo>),
    Magnet(String),
}

impl Source {
    pub fn hash(&self) -> String {
        match self {
            Source::Torrent(_, ti) => hash_of(&ti.info_hashes()),
            Source::Magnet(m) => parse_magnet_uri(m).map(|p| hash_of(&p.info_hashes)).unwrap_or_default(),
        }
    }

    /// The host's key (`InfoHashes::key_hex`, the preferences' `torrent.<key>.*`).
    pub fn key(&self) -> Option<String> {
        match self {
            Source::Torrent(_, ti) => Some(ti.info_hashes().key_hex()),
            Source::Magnet(m) => parse_magnet_uri(m).ok().map(|p| p.info_hashes.key_hex()),
        }
    }

    pub fn from_torrent_bytes(bytes: Vec<u8>) -> Result<Source, CmdError> {
        match TorrentInfo::from_bytes(&bytes) {
            Ok(ti) => Ok(Source::Torrent(bytes, Box::new(ti))),
            Err(_) => Err(err(415, "not a valid torrent file")),
        }
    }

    /// A magnet link, a bare info-hash, or an `http://` / `https://` URL to fetch a torrent
    /// from (which may redirect to a magnet, as indexers do), through the session's proxy when
    /// one is set (`FetchProxy::from_settings`, as RSS feeds).
    pub fn from_url(shared: &Shared, url: &str) -> Result<Source, CmdError> {
        let url = url.trim();
        let magnet = if url.starts_with("magnet:") { Some(url.to_owned()) } else { bare_hash_magnet(url) };
        if let Some(m) = magnet {
            return match parse_magnet_uri(&m) {
                Ok(_) => Ok(Source::Magnet(m)),
                Err(_) => Err(err(400, "invalid magnet link")),
            };
        }
        let proxy = FetchProxy::from_settings(&shared.lock().settings);
        match fetch::fetch(url, proxy) {
            Ok(fetch::Fetched::Torrent(bytes)) => {
                Source::from_torrent_bytes(bytes).map_err(|e| not_fetched(url, &e.msg))
            }
            Ok(fetch::Fetched::Magnet(m)) => match parse_magnet_uri(&m) {
                Ok(_) => Ok(Source::Magnet(m)),
                Err(_) => Err(not_fetched(url, "the URL redirected to an invalid magnet link")),
            },
            Err(fetch::Failed::Url(e)) => Err(err(400, e)),
            Err(fetch::Failed::Net(e)) => Err(not_fetched(url, &e)),
        }
    }
}

/// A URL that gave no torrent: one answer whatever happened (refused, timed out, an HTTP error,
/// not a torrent, a redirect), so the Web UI does not tell which addresses and ports answer
/// (SEC-05, SEC-26). The reason goes to the daemon's own output.
fn not_fetched(url: &str, why: &str) -> CmdError {
    eprintln!("! could not fetch {url}: {why}");
    err(400, format!("could not fetch {url}"))
}

/// A bare info-hash (40 hex or 32 base32 characters) as a magnet link.
pub fn bare_hash_magnet(s: &str) -> Option<String> {
    let hex = s.len() == 40 && s.bytes().all(|c| c.is_ascii_hexdigit());
    let b32 = s.len() == 32 && s.bytes().all(|c| c.is_ascii_alphabetic() || (b'2'..=b'7').contains(&c));
    (hex || b32).then(|| format!("magnet:?xt=urn:btih:{s}"))
}

/// The Add New Torrent dialog's choices for one add, as the JSON body (or the `options` part
/// of the multipart upload) gives them; everything optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddOptions {
    /// Save In; the default download folder when empty.
    pub save: Option<String>,
    /// A new name for the torrent (renames its folder or only file on disk).
    pub name: Option<String>,
    pub label: String,
    /// "Start torrent"; the server's "start paused" preference decides when absent.
    pub start: Option<bool>,
    pub skip_check: bool,
    pub top: bool,
    /// File index to priority 0..7; files not named keep Normal.
    pub priorities: BTreeMap<u32, DownloadPriority>,
    pub sequential: bool,
    pub download_limit: u32,
    pub upload_limit: u32,
    /// The torrent's own seeding goal (the shim's `ratioLimit` / `seedingTimeLimit`).
    pub seed_goal: Option<SeedGoal>,
    /// A magnet: wait for its metadata in the session, then show the dialog
    /// ([`add_awaiting`]); the other choices come with [`finish_awaiting`].
    pub dialog: bool,
}

impl AddOptions {
    pub fn parse(body: &Json) -> Result<AddOptions, &'static str> {
        let text = |k: &str| {
            body.get(k).and_then(Json::as_str).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
        };
        let flag = |k: &str| match body.get(k) {
            None | Some(Json::Null) => Ok(None),
            Some(v) => v.as_bool().map(Some).ok_or("expected a boolean"),
        };
        let mut priorities = BTreeMap::new();
        if let Some(p) = body.get("priorities") {
            for (k, v) in p.entries() {
                let i: u32 = k.parse().map_err(|_| "priorities keys are file indices")?;
                let p = v.as_i64().filter(|p| (0..=7).contains(p)).ok_or("priority must be 0..7")?;
                priorities.insert(i, DownloadPriority(p as u8));
            }
        }
        let limit =
            |k: &str| body.get(k).and_then(Json::as_i64).map_or(0, |v| v.clamp(0, u32::MAX as i64) as u32);
        Ok(AddOptions {
            save: text("save"),
            name: text("name"),
            label: text("label").unwrap_or_default(),
            start: flag("start")?,
            skip_check: flag("skip_check")?.unwrap_or(false),
            top: flag("top")?.unwrap_or(false),
            priorities,
            sequential: flag("sequential")?.unwrap_or(false),
            download_limit: limit("download_limit"),
            upload_limit: limit("upload_limit"),
            seed_goal: None,
            dialog: flag("dialog")?.unwrap_or(false),
        })
    }
}

/// Adds one torrent through the dialog's model: the save folder remembered, the label set,
/// "Add to top of queue" applied when the engine reports it added. Returns the info-hash.
pub fn add(shared: &Shared, source: Source, o: &AddOptions) -> Result<String, CmdError> {
    let label = o.label.trim();
    if !valid_label(label) {
        return Err(err(400, "invalid label"));
    }
    if let Some(save) = &o.save {
        shared.path_policy.check(save).map_err(|why| err(400, why))?;
    }
    if let (Source::Magnet(m), true) = (&source, o.dialog) {
        return add_awaiting(shared, m.clone());
    }
    let (hash, key) = (source.hash(), source.key());
    let mut st = shared.lock();
    let save_path = o.save.clone().unwrap_or_else(|| st.prefs.save_path.clone());
    let file_priorities = match &source {
        Source::Torrent(_, ti) => file_priorities(ti, o)?,
        Source::Magnet(_) if o.priorities.is_empty() => Vec::new(),
        Source::Magnet(_) => return Err(err(400, "priorities name a file the torrent does not have")),
    };
    let choice = AddChoice {
        save_path: save_path.clone(),
        name: o.name.clone(),
        file_priorities,
        start: o.start.unwrap_or(!st.prefs.start_paused),
        skip_hash_check: o.skip_check,
        top_of_queue: o.top,
        label: label.to_owned(),
    };
    // the Preferences' per-torrent defaults first, then the dialog's choices, as the Win32 add does
    let defaults = st.prefs.gui();
    let (params, meta) = match source {
        Source::Torrent(bytes, ti) => {
            let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes), save_path.as_str());
            defaults.apply_to_new(&mut p);
            st.scheduler.apply_to_new(&mut p);
            choice.apply_to(&ti, &mut p);
            (p, Some(meta_of(&ti)))
        }
        Source::Magnet(m) => {
            let mut p = AddTorrentParams::new(TorrentSource::Magnet(m), save_path.as_str());
            defaults.apply_to_new(&mut p);
            st.scheduler.apply_to_new(&mut p);
            // no metadata yet: the choice's flags and name, and no renames
            choice.apply_to(&TorrentInfo::default(), &mut p);
            p.renamed_files.clear();
            (p, None)
        }
    };
    let mut params = params;
    if o.sequential {
        params.flags |= TorrentFlags::SEQUENTIAL_DOWNLOAD;
    }
    params.download_limit = o.download_limit;
    params.upload_limit = o.upload_limit;
    if !label.is_empty() {
        st.set_label_keyed(&hash, key.as_deref(), label);
    }
    if let (Some(goal), Some(key)) = (o.seed_goal, &key) {
        SeedGoal::store_override(&mut st.options, key, Some(goal));
    }
    if let Some(m) = meta {
        st.meta.insert(hash.clone(), m);
    }
    if choice.top_of_queue {
        st.pending_top.insert(hash.clone());
    }
    add::remember_dir(&mut st.options, &save_path);
    shared.save(&mut st);
    // a `.torrent`'s copy is stored once the engine has it (Directories > Store .torrents in)
    let folders = st.prefs.torrent_folders.clone();
    st.torrent_files.adding(&folders, &params);
    drop(st);
    shared.handle.add_torrent(params);
    // Ask for the new torrent's status now rather than at the next refresh.
    shared.handle.post_torrent_updates();
    Ok(hash)
}

/// The dialog's priorities for every file of `ti` (Normal where not named, pad files never);
/// empty when none were given.
fn file_priorities(ti: &TorrentInfo, o: &AddOptions) -> Result<Vec<DownloadPriority>, CmdError> {
    if o.priorities.is_empty() {
        return Ok(Vec::new());
    }
    let fs = ti.files();
    if o.priorities.keys().any(|&i| i as usize >= fs.num_files()) {
        return Err(err(400, "priorities name a file the torrent does not have"));
    }
    Ok((0..fs.num_files())
        .map(|i| match fs.pad_file_at(i) {
            true => DownloadPriority::DONT_DOWNLOAD,
            false => o.priorities.get(&(i as u32)).copied().unwrap_or(DownloadPriority::NORMAL),
        })
        .collect())
}

/// A magnet for the Add dialog: it joins the session in upload mode (fetching its metadata,
/// writing nothing) and outside the queue; once the engine hands out its file list the tick's
/// `adds` changes and `GET /pa/add` lists it as `ready`. Returns the info-hash.
pub fn add_awaiting(shared: &Shared, uri: String) -> Result<String, CmdError> {
    let m = parse_magnet_uri(&uri).map_err(|_| err(400, "invalid magnet link"))?;
    let (hash, key) = (hash_of(&m.info_hashes), m.info_hashes.key_hex());
    let mut st = shared.lock();
    if st.by_hash.contains_key(&hash) {
        return Err(err(409, "the torrent is already in the list"));
    }
    let save_path = st.prefs.save_path.clone();
    let mut p = AddTorrentParams::new(TorrentSource::Magnet(uri), save_path.as_str());
    st.prefs.gui().apply_to_new(&mut p);
    st.scheduler.apply_to_new(&mut p);
    AwaitingMetadata::prepare(&mut p);
    let s = &mut *st;
    s.torrent_files.adding(&s.prefs.torrent_folders, &p);
    s.awaiting.insert(&mut s.options, &key, &save_path);
    s.adds_version += 1;
    shared.save(&mut st);
    drop(st);
    shared.handle.add_torrent(p);
    shared.handle.post_torrent_updates();
    Ok(hash)
}

/// The waiting magnet whose torrent has `hash`, once its metadata is known: its key.
fn ready_key(st: &State, hash: &str) -> Option<String> {
    st.awaiting.ready().into_iter().find(|k| {
        let id = st.awaiting.get(k).and_then(|w| w.id);
        id.and_then(|id| st.torrents.get(&id)).is_some_and(|e| e.hash == hash)
    })
}

/// `GET /pa/add`'s `ready`: the magnets whose dialog can open, as `{"hash", "name"}`.
pub fn ready_adds(st: &State) -> Json {
    let mut out = Vec::new();
    for k in st.awaiting.ready() {
        let Some(w) = st.awaiting.get(&k) else { continue };
        let (Some(id), Some(ti)) = (w.id, &w.ti) else { continue };
        let Some(e) = st.torrents.get(&id) else { continue };
        out.push(Json::obj().with("hash", e.hash.as_str()).with("name", ti.name()));
    }
    Json::Arr(out)
}

/// `GET /pa/torrent-info?hash=`: the dialog's Torrent Contents for a waiting magnet.
pub fn awaiting_info(st: &State, hash: &str) -> Result<Json, CmdError> {
    let key = ready_key(st, hash).ok_or_else(|| err(404, "no magnet waiting for the Add dialog"))?;
    let ti = st.awaiting.get(&key).and_then(|w| w.ti.clone()).expect("ready");
    Ok(torrent_info(&ti))
}

/// `POST /pa/add` with a waiting magnet's `hash`: OK gives the torrent the dialog's choices
/// (`frontend::add::AddChoice::apply_to_torrent`); `cancel` removes it with its part file.
pub fn finish_awaiting(shared: &Shared, hash: &str, o: &AddOptions, cancel: bool) -> Result<(), CmdError> {
    let label = o.label.trim();
    if !valid_label(label) {
        return Err(err(400, "invalid label"));
    }
    if let Some(save) = &o.save {
        shared.path_policy.check(save).map_err(|why| err(400, why))?;
    }
    let mut st = shared.lock();
    let key = ready_key(&st, hash).ok_or_else(|| err(404, "no magnet waiting for the Add dialog"))?;
    let w = st.awaiting.get(&key).cloned().expect("ready");
    let (id, ti) = (w.id.expect("ready"), w.ti.expect("ready"));
    let h = shared.handle.torrent(id);
    if cancel {
        let s = &mut *st;
        s.awaiting.finish(&mut s.options, &key);
        s.adds_version += 1;
        shared.save(&mut st);
        drop(st);
        shared.handle.remove_torrent(id, RemoveFlags::DELETE_PARTFILE);
        return Ok(());
    }
    let save_path = o.save.clone().unwrap_or_else(|| w.save_path.clone());
    let choice = AddChoice {
        save_path: save_path.clone(),
        name: o.name.clone(),
        file_priorities: file_priorities(&ti, o)?,
        start: o.start.unwrap_or(!st.prefs.start_paused),
        skip_hash_check: false,
        top_of_queue: o.top,
        label: label.to_owned(),
    };
    if !label.is_empty() {
        st.set_label(hash, label);
    }
    st.meta.insert(hash.to_owned(), meta_of(&ti));
    let seed_only = st.scheduler.mode() == Mode::SeedOnly;
    let s = &mut *st;
    s.awaiting.finish(&mut s.options, &key);
    s.adds_version += 1;
    add::remember_dir(&mut s.options, &save_path);
    shared.save(&mut st);
    drop(st);
    if o.sequential {
        h.set_flags(TorrentFlags::SEQUENTIAL_DOWNLOAD);
    }
    if o.download_limit > 0 {
        h.set_download_limit(o.download_limit);
    }
    if o.upload_limit > 0 {
        h.set_upload_limit(o.upload_limit);
    }
    choice.apply_to_torrent(&h, &ti, &w.save_path, seed_only);
    shared.handle.post_torrent_updates();
    Ok(())
}

/// The private flag, comment, creator and creation date of torrent `id` for the Properties
/// pages (`torrents/properties`, the General tab): from [`State::meta`], else asked of the
/// engine (`TorrentHandle::metadata`, which also knows torrents restored from the state
/// directory) and kept there, so each torrent is asked once. The default (not private, no
/// text) while the torrent has no metadata, or when the engine cannot say.
pub fn meta(shared: &Shared, id: TorrentId) -> Meta {
    let hash = {
        let st = shared.lock();
        let Some(e) = st.torrents.get(&id) else { return Meta::default() };
        if let Some(m) = st.meta.get(&e.hash) {
            return m.clone();
        }
        if !e.status.has_metadata {
            return Meta::default();
        }
        e.hash.clone()
    };
    let Some(m) = shared.handle.torrent(id).metadata() else { return Meta::default() };
    let m = Meta {
        comment: m.comment,
        created_by: m.created_by,
        creation_date: m.creation_date,
        private: m.private,
    };
    let mut st = shared.lock();
    if st.by_hash.get(&hash) == Some(&id) {
        // the list's `private` field changes with it
        if m.private {
            st.touch(&hash);
        }
        st.meta.insert(hash, m.clone());
    }
    m
}

pub fn meta_of(ti: &TorrentInfo) -> Meta {
    Meta {
        comment: ti.comment().to_owned(),
        created_by: ti.creator().to_owned(),
        creation_date: ti.creation_date(),
        private: ti.is_private(),
    }
}

/// The Save In list: the default download folder, then the recent ones.
pub fn save_in(st: &State) -> Vec<String> {
    add::save_in_choices(&st.prefs.save_path, &add::recent_dirs(&st.options))
}

/// `/pa/torrent-info`: the Add dialog's Torrent Contents for a `.torrent` without adding it:
/// name, comment, created_by, size (bytes and text), date (Unix and text), private, hash, and
/// the files as `{"i": engine index, "path": below the torrent's folder, "size", "text"}`, pad
/// files left out, so the page builds the same tree `frontend::add::FileTree` would.
pub fn torrent_info(ti: &TorrentInfo) -> Json {
    let fs = ti.files();
    let mut files = Vec::new();
    for i in 0..fs.num_files() {
        if fs.pad_file_at(i) {
            continue;
        }
        let path = fs.file_path(i, "");
        let mut parts: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
        if parts.len() > 1 {
            parts.remove(0);
        }
        files.push(
            Json::obj()
                .with("i", i)
                .with("path", parts.join("/"))
                .with("size", fs.file_size(i))
                .with("text", fmt::size(fs.file_size(i))),
        );
    }
    let total: u64 = (0..fs.num_files()).filter(|&i| !fs.pad_file_at(i)).map(|i| fs.file_size(i)).sum();
    Json::obj()
        .with("name", ti.name())
        .with("comment", ti.comment())
        .with("created_by", ti.creator())
        .with("size", total)
        .with("size_text", fmt::size(total))
        .with("date", ti.creation_date().map_or(Json::Null, Json::from))
        .with("date_text", ti.creation_date().map(fmt::date_time).unwrap_or_default())
        .with("private", ti.is_private())
        .with("hash", hash_of(&ti.info_hashes()))
        .with("files", Json::Arr(files))
}

// ---- the session ---------------------------------------------------------------------------

/// `GET /pa/session/limits`: the global limits in bytes per second (0 unlimited) and whether
/// the alternative ones are in force.
pub fn session_limits(st: &State) -> Json {
    Json::obj()
        .with("down", st.prefs.dl_limit)
        .with("up", st.prefs.up_limit)
        .with("alt", st.prefs.alt_enabled)
        .with("alt_down", st.prefs.alt_dl_limit)
        .with("alt_up", st.prefs.alt_up_limit)
}

/// `POST /pa/session/limits`: `alt` switches the alternative limits on or off, `down` / `up`
/// set the limits of the mode in force after that, `alt_down` / `alt_up` the alternative ones
/// explicitly; each optional.
pub fn set_session_limits(shared: &Shared, body: &Json) -> Result<(), CmdError> {
    let rate = |k: &str| match body.get(k) {
        None | Some(Json::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .filter(|&n| n >= 0)
            .map(|n| Some(n.min(u32::MAX as i64) as u32))
            .ok_or(err(400, format!("{k} must be a non-negative number"))),
    };
    let (down, up, alt_down, alt_up) = (rate("down")?, rate("up")?, rate("alt_down")?, rate("alt_up")?);
    let alt = match body.get("alt") {
        None | Some(Json::Null) => None,
        Some(v) => Some(v.as_bool().ok_or(err(400, "alt must be a boolean"))?),
    };
    let mut st = shared.lock();
    if st.hosted {
        // no alternative limits; the normal ones take the Preferences' path, so the host gets
        // them as `lt.download_rate_limit` / `lt.upload_rate_limit`
        if alt == Some(true) || alt_down.is_some() || alt_up.is_some() {
            return Err(err(409, prefs::HOSTED_NO_ALT));
        }
        return set_normal_limits(shared, &mut st, down, up);
    }
    if let Some(a) = alt {
        st.prefs.alt_enabled = a;
    }
    let in_alt = st.prefs.alt_enabled;
    if let Some(d) = down {
        if in_alt { st.prefs.alt_dl_limit = d } else { st.prefs.dl_limit = d }
    }
    if let Some(u) = up {
        if in_alt { st.prefs.alt_up_limit = u } else { st.prefs.up_limit = u }
    }
    if let Some(d) = alt_down {
        st.prefs.alt_dl_limit = d;
    }
    if let Some(u) = alt_up {
        st.prefs.alt_up_limit = u;
    }
    shared.apply_limits(&mut st);
    Ok(())
}

/// Hosted: the normal global limits set through the Preferences' commit (stored in the mirror
/// and journaled to the host, nothing sent to the engine).
pub fn set_normal_limits(
    shared: &Shared,
    st: &mut State,
    down: Option<u32>,
    up: Option<u32>,
) -> Result<(), CmdError> {
    let changes: Vec<prefs::Change> = [("download_rate_limit", down), ("upload_rate_limit", up)]
        .into_iter()
        .filter_map(|(id, v)| {
            Some((prefs::find(id)?, patorrent_frontend::settings::Value::Int(i64::from(v?))))
        })
        .collect();
    if changes.is_empty() {
        return Ok(());
    }
    prefs::commit(shared, st, &changes, None).map_err(|(field, why)| err(400, format!("{field}: {why}")))
}

// ---- the native endpoints -------------------------------------------------------------------

fn json_error(e: CmdError) -> Response {
    Response::new(e.status, "application/json", Json::obj().with("error", e.msg.as_str()).to_json())
}

fn no_content() -> Response {
    Response::new(204, "", "")
}

/// The CSRF rule of the native commands: the custom header, besides the shim's `Origin` check
/// (which `api.rs` applies to every `/pa/` request).
pub fn csrf_header_ok(head: &Head) -> bool {
    head.header(CSRF_HEADER).is_some_and(|v| v.trim().eq_ignore_ascii_case(CSRF_VALUE))
}

/// The JSON body of a native command (an object), or the error response.
fn json_body(req: &Request) -> Result<Json, CmdError> {
    let (kind, _) = parse_header_params(req.head.header("content-type").unwrap_or(""));
    if !req.body.is_empty() && kind != "application/json" {
        return Err(err(415, "expected application/json"));
    }
    if req.body.is_empty() {
        return Ok(Json::obj());
    }
    let text = std::str::from_utf8(&req.body).map_err(|_| err(400, "body is not UTF-8"))?;
    match json::parse(text) {
        Ok(v @ Json::Obj(_)) => Ok(v),
        Ok(_) => Err(err(400, "expected a JSON object")),
        Err(e) => Err(err(400, format!("bad json: {e}"))),
    }
}

/// A multipart upload: the `.torrent` parts (any part with a file name) and the JSON
/// `options` part, if one.
fn multipart_body(req: &Request) -> Result<(Vec<Part>, Json), CmdError> {
    let ct = req.head.header("content-type").unwrap_or("");
    let boundary = multipart_boundary(ct).ok_or_else(|| err(415, "expected multipart/form-data"))?;
    let parts = parse_multipart(&req.body, &boundary).map_err(|e| err(400, e))?;
    let mut files = Vec::new();
    let mut options = Json::obj();
    for part in parts {
        if part.filename.is_some() {
            files.push(part);
        } else if part.name == "options" {
            let text = String::from_utf8_lossy(&part.data);
            options = match json::parse(&text) {
                Ok(v @ Json::Obj(_)) => v,
                _ => return Err(err(400, "options must be a JSON object")),
            };
        }
    }
    Ok((files, options))
}

/// Answers a `/pa/...` request (`endpoint` is the path after `/pa/`), authenticated already.
/// `key` is the requester's session.
pub fn serve(shared: &Shared, endpoint: &str, req: &Request, key: &str) -> Response {
    match serve_inner(shared, endpoint, req, key) {
        Ok(r) => r,
        Err(e) => json_error(e),
    }
}

fn serve_inner(shared: &Shared, endpoint: &str, req: &Request, key: &str) -> Result<Response, CmdError> {
    let read = matches!(req.head.method.as_str(), "GET" | "HEAD");
    // the RSS Downloader's own (`rss.rs`)
    if let Some(rest) = endpoint.strip_prefix("rss").filter(|r| r.is_empty() || r.starts_with('/')) {
        if read {
            return super::rss::serve(shared, rest, req, None);
        }
        if !csrf_header_ok(&req.head) {
            return Err(err(403, format!("missing X-Requested-With: {CSRF_VALUE} header")));
        }
        return super::rss::serve(shared, rest, req, Some(json_body(req)?));
    }
    if read {
        return match endpoint {
            "session/limits" => Ok(Response::json(&session_limits(&shared.lock()))),
            "add" => {
                let st = shared.lock();
                let labels: Vec<Json> = st.categories.keys().map(|l| Json::Str(l.clone())).collect();
                let dirs: Vec<Json> = save_in(&st).into_iter().map(Json::Str).collect();
                Ok(Response::json(
                    &Json::obj()
                        .with("save_in", Json::Arr(dirs))
                        .with("labels", Json::Arr(labels))
                        .with("start", !st.prefs.start_paused)
                        .with("show_dialog", st.prefs.show_add_dialog)
                        .with("ready", ready_adds(&st)),
                ))
            }
            "torrent-info" => {
                let q = parse_urlencoded(req.head.query.as_bytes());
                let hash = q.iter().find(|(k, _)| k == "hash").map(|(_, v)| v.as_str()).unwrap_or_default();
                Ok(Response::json(&awaiting_info(&shared.lock(), hash)?))
            }
            "prefs" => {
                let st = shared.lock();
                Ok(Response::json(&prefs::model(shared, &st)))
            }
            "torrents/properties" => {
                let sel = query_selection(&parse_urlencoded(req.head.query.as_bytes()))?;
                Ok(Response::json(&properties_reply(shared, &sel)?))
            }
            _ => Err(err(405, "POST only")),
        };
    }
    if !csrf_header_ok(&req.head) {
        return Err(err(403, format!("missing X-Requested-With: {CSRF_VALUE} header")));
    }
    match endpoint {
        "add" => {
            let (kind, _) = parse_header_params(req.head.header("content-type").unwrap_or(""));
            if kind == "multipart/form-data" {
                let (files, options) = multipart_body(req)?;
                let o = AddOptions::parse(&options).map_err(|e| err(400, e))?;
                if files.is_empty() {
                    return Err(err(400, "no torrent file in the upload"));
                }
                for f in files {
                    add(shared, Source::from_torrent_bytes(f.data)?, &o)?;
                }
                return Ok(no_content());
            }
            let body = json_body(req)?;
            let o = AddOptions::parse(&body).map_err(|e| err(400, e))?;
            if let Some(hash) = body.get("hash").and_then(Json::as_str) {
                let cancel = body.get("cancel").and_then(Json::as_bool).unwrap_or(false);
                finish_awaiting(shared, hash, &o, cancel)?;
                return Ok(no_content());
            }
            let url = body
                .get("url")
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .ok_or_else(|| err(400, "missing url"))?;
            let hash = add(shared, Source::from_url(shared, url)?, &o)?;
            if o.dialog {
                return Ok(Response::json(&Json::obj().with("hash", hash)));
            }
            Ok(no_content())
        }
        "torrent-info" => {
            let (files, _) = multipart_body(req)?;
            let f = files.into_iter().next().ok_or_else(|| err(400, "no torrent file in the upload"))?;
            let Source::Torrent(_, ti) = Source::from_torrent_bytes(f.data)? else { unreachable!() };
            Ok(Response::json(&torrent_info(&ti)))
        }
        "session/limits" => {
            set_session_limits(shared, &json_body(req)?)?;
            Ok(no_content())
        }
        "session/shutdown" => {
            shared.lock().shutdown = true;
            // hosted: the host exits (told without the lock)
            if let Some(host) = shared.hosted() {
                host.shutdown();
            }
            Ok(no_content())
        }
        "prefs" => {
            // a partial object of ids to values; 400 names the first bad field
            let body = json_body(req)?;
            let changes = prefs::parse(&body).map_err(|(field, why)| err(400, format!("{field}: {why}")))?;
            prefs::commit_request(shared, &changes, key, |f| patorrent_core::IpFilter::load(f))
                .map_err(|(field, why)| err(400, format!("{field}: {why}")))?;
            Ok(no_content())
        }
        _ => {
            let Some(cmd) = endpoint.strip_prefix("torrents/") else {
                return Err(err(404, "no such endpoint"));
            };
            let body = json_body(req)?;
            torrent_command(shared, cmd, &body)?;
            Ok(no_content())
        }
    }
}

/// One `/pa/torrents/<cmd>`.
fn torrent_command(shared: &Shared, cmd: &str, body: &Json) -> Result<(), CmdError> {
    // the per-torrent ones
    match cmd {
        "files" => {
            let id = one(&shared.lock(), body)?;
            let mut prios = Vec::new();
            for (k, v) in body.get("priorities").map(Json::entries).unwrap_or_default() {
                let i: u32 = k.parse().map_err(|_| err(400, "priorities keys are file indices"))?;
                let p = v
                    .as_i64()
                    .filter(|p| (0..=7).contains(p))
                    .ok_or_else(|| err(400, "priority must be 0..7"))?;
                prios.push((i, DownloadPriority(p as u8)));
            }
            if prios.is_empty() {
                return Err(err(400, "missing priorities"));
            }
            return files(shared, id, &prios);
        }
        "rename" => {
            let id = one(&shared.lock(), body)?;
            let (Some(old), Some(new)) =
                (body.get("old").and_then(Json::as_str), body.get("new").and_then(Json::as_str))
            else {
                return Err(err(400, "missing old or new"));
            };
            let folder = body.get("folder").and_then(Json::as_bool);
            return rename(shared, id, old, new, folder);
        }
        _ => {}
    }
    let sel = Selection::parse(body).map_err(|e| err(400, e))?;
    let ids = sel.resolve(&shared.lock());
    if ids.is_empty() {
        return match sel {
            Selection::Ids(_) => Err(not_found()),
            Selection::All(_) => Ok(()),
        };
    }
    let flag = |k: &str| body.get(k).and_then(Json::as_bool).unwrap_or(false);
    let rate = |k: &str| -> Result<Option<u32>, CmdError> {
        match body.get(k) {
            None | Some(Json::Null) => Ok(None),
            Some(v) => v
                .as_i64()
                .filter(|&n| n >= 0)
                .map(|n| Some(n.min(u32::MAX as i64) as u32))
                .ok_or_else(|| err(400, format!("{k} must be a non-negative number"))),
        }
    };
    match cmd {
        "start" => start(shared, &ids),
        "force-start" => force_start(shared, &ids),
        "pause" => pause(shared, &ids),
        "stop" => stop(shared, &ids),
        "recheck" => recheck(shared, &ids),
        "reannounce" => reannounce(shared, &ids),
        "remove" => remove(shared, &ids, flag("data")),
        "queue" => {
            let m = body
                .get("move")
                .and_then(Json::as_str)
                .and_then(queue_move_from)
                .ok_or_else(|| err(400, "move must be up, down, top or bottom"))?;
            queue(shared, &ids, m);
        }
        "limits" => {
            let (down, up) = (rate("down")?, rate("up")?);
            if down.is_none() && up.is_none() {
                return Err(err(400, "missing down or up"));
            }
            limits(shared, &ids, down, up);
        }
        "label" => label(shared, &ids, body.get("label").and_then(Json::as_str).unwrap_or(""))?,
        "location" => location(shared, &ids, body.get("path").and_then(Json::as_str).unwrap_or(""))?,
        "super-seed" => super_seed(shared, &ids, body.get("on").and_then(Json::as_bool).unwrap_or(true)),
        "properties" => set_properties(shared, &ids, body.get("fields").unwrap_or(&Json::obj()))?,
        _ => return Err(err(404, "no such command")),
    }
    Ok(())
}

/// Hashes the alert loop applies "Add to top of queue" to when their torrent shows up.
pub(crate) type PendingTop = HashSet<String>;

#[cfg(test)]
mod tests {
    use super::*;
    use patorrent_core::{InfoHashes, Sha1Hash, TorrentStatus};

    fn state_with(names: &[(&str, &str, Option<u32>, bool)]) -> (State, Vec<String>) {
        let mut st = State::for_tests();
        let mut hashes = Vec::new();
        for (i, (name, label, pos, finished)) in names.iter().enumerate() {
            let hash = Sha1Hash([i as u8 + 1; 20]);
            let s = TorrentStatus {
                id: TorrentId::from_raw(i as u64 + 1),
                info_hashes: InfoHashes::v1(hash),
                name: (*name).to_owned(),
                queue_position: *pos,
                is_finished: *finished,
                ..TorrentStatus::default()
            };
            st.update(s, 1);
            if !label.is_empty() {
                st.torrent_categories.insert(hash.to_hex(), (*label).to_owned());
            }
            hashes.push(hash.to_hex());
        }
        (st, hashes)
    }

    #[test]
    fn parses_selections() {
        let (st, h) = state_with(&[
            ("Alpha", "tv", Some(0), false),
            ("beta", "", Some(1), true),
            ("Gamma", "tv", None, true),
        ]);
        let ids = |v: Vec<TorrentId>| {
            let mut v: Vec<u64> = v.iter().map(|id| id.to_raw()).collect();
            v.sort_unstable();
            v
        };
        let sel = Selection::parse(
            &json::parse(&format!(r#"{{"ids":["{}","{}","nope"]}}"#, h[0].to_uppercase(), h[2])).unwrap(),
        )
        .unwrap();
        assert_eq!(sel, Selection::Ids(vec![h[0].clone(), h[2].clone(), "nope".into()]));
        assert_eq!(ids(sel.resolve(&st)), [1, 3], "unknown ids are skipped, hex is case-insensitive");
        for bad in [
            r#"{}"#,
            r#"{"ids":[]}"#,
            r#"{"ids":"x"}"#,
            r#"{"ids":[1]}"#,
            r#"{"all":true,"filter":{"category":"zzz"}}"#,
        ] {
            assert!(Selection::parse(&json::parse(bad).unwrap()).is_err(), "{bad}");
        }
        // "all" with the view's filter: category, label, search, and nothing at all
        let all = |f: &str| {
            Selection::parse(&json::parse(&format!(r#"{{"all":true,"filter":{f}}}"#)).unwrap())
                .unwrap()
                .resolve(&st)
        };
        assert_eq!(ids(all(r#"{"category":"all","label":null,"search":""}"#)), [1, 2, 3]);
        assert_eq!(ids(all(r#"{"category":"completed"}"#)), [2, 3]);
        assert_eq!(ids(all(r#"{"category":"downloading"}"#)), [1]);
        assert_eq!(ids(all(r#"{"label":"tv"}"#)), [1, 3]);
        assert_eq!(ids(all(r#"{"label":""}"#)), [2], "the empty label is \"no label\"");
        assert_eq!(ids(all(r#"{"label":"tv","search":" GAM "}"#)), [3]);
        assert_eq!(ids(all(r#"{"category":"completed","search":"a"}"#)), [2, 3]);
        assert_eq!(
            ids(Selection::parse(&json::parse(r#"{"all":true}"#).unwrap()).unwrap().resolve(&st)),
            [1, 2, 3]
        );
        // "all" wins over ids
        assert!(matches!(
            Selection::parse(&json::parse(r#"{"all":true,"ids":["x"]}"#).unwrap()).unwrap(),
            Selection::All(_)
        ));
        assert_eq!(
            one(&st, &json::parse(&format!(r#"{{"id":"{}"}}"#, h[1])).unwrap()),
            Ok(TorrentId::from_raw(2))
        );
        assert_eq!(one(&st, &json::parse(r#"{"id":"00"}"#).unwrap()).unwrap_err().status, 404);
        assert_eq!(one(&st, &json::parse(r#"{}"#).unwrap()).unwrap_err().status, 400);
    }

    #[test]
    fn queue_moves_keep_the_relative_order() {
        let (st, _) = state_with(&[
            ("a", "", Some(2), false),
            ("b", "", Some(0), false),
            ("c", "", None, true),
            ("d", "", Some(1), false),
        ]);
        let all: Vec<TorrentId> = (1..=4).map(TorrentId::from_raw).collect();
        let raw = |v: Vec<TorrentId>| v.iter().map(|id| id.to_raw()).collect::<Vec<_>>();
        assert_eq!(
            raw(queue_order(&st, &all, QueueMove::Up)),
            [2, 4, 1],
            "lowest position first; c is outside the queue"
        );
        assert_eq!(raw(queue_order(&st, &all, QueueMove::Bottom)), [2, 4, 1]);
        assert_eq!(raw(queue_order(&st, &all, QueueMove::Down)), [1, 4, 2]);
        assert_eq!(raw(queue_order(&st, &all, QueueMove::Top)), [1, 4, 2]);
        assert_eq!(queue_move_from("top"), Some(QueueMove::Top));
        assert_eq!(queue_move_from("sideways"), None);
    }

    #[test]
    fn parses_add_options() {
        let o = AddOptions::parse(&json::parse(r#"{"save":" D:\\in ","name":"New","label":"tv","start":false,"skip_check":true,"top":true,"priorities":{"0":7,"2":0},"download_limit":1024}"#).unwrap()).unwrap();
        assert_eq!(o.save.as_deref(), Some("D:\\in"));
        assert_eq!(o.name.as_deref(), Some("New"));
        assert_eq!((o.label.as_str(), o.start, o.skip_check, o.top), ("tv", Some(false), true, true));
        assert_eq!(
            o.priorities,
            BTreeMap::from([(0, DownloadPriority::TOP), (2, DownloadPriority::DONT_DOWNLOAD)])
        );
        assert_eq!((o.download_limit, o.upload_limit, o.sequential), (1024, 0, false));
        let o = AddOptions::parse(&json::parse(r#"{"save":"","start":null}"#).unwrap()).unwrap();
        assert_eq!(o, AddOptions::default());
        for bad in [r#"{"start":"maybe"}"#, r#"{"priorities":{"x":1}}"#, r#"{"priorities":{"1":9}}"#] {
            assert!(AddOptions::parse(&json::parse(bad).unwrap()).is_err(), "{bad}");
        }
        assert_eq!(
            bare_hash_magnet(&"a".repeat(40)),
            Some(format!("magnet:?xt=urn:btih:{}", "a".repeat(40)))
        );
        assert_eq!(bare_hash_magnet("xyz"), None);
        assert!(valid_label("tv/hd") && !valid_label("/tv") && !valid_label("a//b") && valid_label(""));
        assert!(
            valid_new_path("a/b.txt")
                && !valid_new_path("a/../b")
                && !valid_new_path("c:x")
                && !valid_new_path("")
        );
        assert_eq!(api_path("\\x\\y\\"), "x/y");
    }

    #[test]
    fn torrent_info_lists_the_files_below_the_root() {
        let multi = b"d4:infod5:filesld6:lengthi3e4:pathl1:x5:y.txteed6:lengthi4e4:pathl5:z.txteee4:name4:root12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        let ti = TorrentInfo::from_bytes(multi).unwrap();
        let v = torrent_info(&ti);
        assert_eq!(v.get("name"), Some(&Json::from("root")));
        assert_eq!(v.get("size"), Some(&Json::from(7u64)));
        assert_eq!(v.get("size_text"), Some(&Json::from("7 B")));
        assert_eq!(v.get("date"), Some(&Json::Null));
        let Some(Json::Arr(files)) = v.get("files") else { panic!() };
        assert_eq!(files[0].get("path"), Some(&Json::from("x/y.txt")));
        assert_eq!(files[0].get("i"), Some(&Json::from(0usize)));
        assert_eq!(files[1].get("path"), Some(&Json::from("z.txt")));
        assert_eq!(files[1].get("size"), Some(&Json::from(4u64)));
        let single =
            b"d4:infod6:lengthi5e4:name5:a.iso12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        let v = torrent_info(&TorrentInfo::from_bytes(single).unwrap());
        let Some(Json::Arr(files)) = v.get("files") else { panic!() };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].get("path"), Some(&Json::from("a.iso")));
    }

    #[test]
    fn the_csrf_header() {
        let head = |v: Option<&str>| Head {
            method: "POST".into(),
            headers: v.map(|v| (CSRF_HEADER.to_owned(), v.to_owned())).into_iter().collect(),
            ..Head::default()
        };
        assert!(csrf_header_ok(&head(Some("PaTorrent"))));
        assert!(csrf_header_ok(&head(Some(" patorrent "))));
        assert!(!csrf_header_ok(&head(Some("XMLHttpRequest"))));
        assert!(!csrf_header_ok(&head(None)));
    }
}
