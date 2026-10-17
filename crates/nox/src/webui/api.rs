//! Request checks (Host, CSRF, login) and the WebAPI v2 endpoints.
//!
//! Written from qBittorrent's published WebAPI documentation ("WebUI API (qBittorrent 5.0)"):
//! names, parameters, units and status codes follow it. Where it is silent the choice is ours
//! and noted in CHANGELOG.md.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

use patorrent_core::metainfo::{TorrentInfo, parse_magnet_uri};
use patorrent_core::{
    AddTorrentParams, DownloadPriority, EncPolicy, ProxyType, RemoveFlags, TorrentFlags, TorrentId,
    TorrentSource, TrackerStatus, VERSION,
};

use super::auth::{PasswordHash, constant_time_eq, random_hex};
use super::http::{
    Head, Part, Response, cookie, multipart_boundary, parse_header_params, parse_multipart, parse_urlencoded,
};
use super::json::{self, Json};
use super::torrent::{self, Extra};
use super::{ClientSession, MAX_SESSIONS, Meta, Reply, ReplyKind, Shared, State, fetch};

/// The qBittorrent release whose WebAPI (2.11.2) this implements; clients pick features by it.
pub const COMPAT_VERSION: &str = "v5.0.0";
pub const WEBAPI_VERSION: &str = "2.11.2";
/// Largest body accepted before a client has logged in (the login form).
pub const UNAUTHENTICATED_BODY: usize = 64 * 1024;

const PAGE: &str = include_str!("page.html");
const SCRIPT: &str = include_str!("app.js");

/// A complete request.
pub struct Request {
    pub head: Head,
    pub body: Vec<u8>,
    pub peer: IpAddr,
}

/// Request parameters: the query string, then a urlencoded or multipart body.
struct Params {
    pairs: Vec<(String, String)>,
    files: Vec<Part>,
}

impl Params {
    fn get(&self, name: &str) -> Option<&str> {
        self.pairs.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn flag(&self, name: &str) -> bool {
        self.get(name).is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
    }

    fn int(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(|v| v.trim().parse().ok())
    }
}

fn params(req: &Request) -> Result<Params, Response> {
    let mut p = Params { pairs: parse_urlencoded(req.head.query.as_bytes()), files: Vec::new() };
    if req.body.is_empty() {
        return Ok(p);
    }
    let ct = req.head.header("content-type").unwrap_or("");
    let (kind, _) = parse_header_params(ct);
    if kind == "multipart/form-data" {
        let boundary = multipart_boundary(ct).ok_or_else(|| Response::text(400, "bad multipart boundary"))?;
        let parts = parse_multipart(&req.body, &boundary).map_err(|e| Response::text(400, e))?;
        for part in parts {
            if part.filename.is_some() {
                p.files.push(part);
            } else {
                p.pairs.push((part.name, String::from_utf8_lossy(&part.data).into_owned()));
            }
        }
    } else {
        // application/x-www-form-urlencoded, and bodies without a content type
        p.pairs.extend(parse_urlencoded(&req.body));
    }
    Ok(p)
}

/// The IP address without its IPv4-mapped IPv6 wrapping.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// The host part of a `Host` header (`name`, `1.2.3.4`, `[::1]`, each with an optional port).
fn host_part(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("");
    }
    match authority.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    }
}

fn wildcard_match(pattern: &str, s: &str) -> bool {
    let p = pattern.as_bytes();
    let s = s.as_bytes();
    // iterative glob with backtracking to the last `*`
    let (mut pi, mut si, mut star, mut mark) = (0, 0, None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi].eq_ignore_ascii_case(&s[si])) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

/// Host header validation: an IP address, `localhost`, or a name in the domain list.
pub fn host_allowed(host: Option<&str>, domain_list: &str) -> bool {
    let Some(host) = host else { return true }; // HTTP/1.0 without Host
    let name = host_part(host.trim());
    if name.parse::<Ipv4Addr>().is_ok()
        || name.parse::<Ipv6Addr>().is_ok()
        || name.eq_ignore_ascii_case("localhost")
    {
        return true;
    }
    domain_list.split([';', ',']).map(str::trim).filter(|d| !d.is_empty()).any(|d| wildcard_match(d, name))
}

fn normalize_authority(a: &str) -> String {
    let a = a.trim().to_ascii_lowercase();
    a.strip_suffix(":80").or_else(|| a.strip_suffix(":443")).map(str::to_owned).unwrap_or(a)
}

/// The `host[:port]` of an origin or absolute URL.
fn url_authority(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    Some(auth.rsplit_once('@').map_or(auth, |(_, h)| h))
}

/// CSRF protection: an `Origin` or (without one) a `Referer` must name the host the request was
/// sent to. Requests with neither (scripts, curl) pass.
pub fn csrf_ok(head: &Head) -> bool {
    let host = normalize_authority(head.header("host").unwrap_or(""));
    let check = |url: &str| url_authority(url).is_some_and(|a| normalize_authority(a) == host);
    if let Some(origin) = head.header("origin") {
        return check(origin);
    }
    if let Some(referer) = head.header("referer") {
        return check(referer);
    }
    true
}

/// The session key of an authenticated request: its SID, or a fixed key for loopback clients
/// when the local-auth bypass is on.
fn authenticate(st: &mut State, head: &Head, ip: IpAddr, now: Instant) -> Option<String> {
    let timeout = Duration::from_secs(st.prefs.session_timeout.max(1));
    if let Some(sid) = head.header("cookie").and_then(|c| cookie(c, "SID")) {
        if let Some(s) = st.sessions.get_mut(sid) {
            if now.duration_since(s.last_used) < timeout {
                s.last_used = now;
                return Some(sid.to_owned());
            }
            st.sessions.remove(sid);
        }
    }
    if st.is_loopback_bypass(ip) {
        let key = "local".to_owned();
        st.sessions
            .entry(key.clone())
            .or_insert(ClientSession { last_used: now, maindata: None })
            .last_used = now;
        return Some(key);
    }
    None
}

/// Whether a request may carry a large body (it is authenticated).
pub fn authorized(shared: &Shared, head: &Head, ip: IpAddr) -> bool {
    let mut st = shared.lock();
    authenticate(&mut st, head, canonical(ip), Instant::now()).is_some()
}

fn with_security_headers(r: Response) -> Response {
    r.with_header("X-Frame-Options", "SAMEORIGIN")
        .with_header("X-Content-Type-Options", "nosniff")
        .with_header("Referrer-Policy", "same-origin")
        .with_header("Cache-Control", "no-store")
}

/// Answers one request.
pub fn serve(shared: &Shared, req: &Request) -> Response {
    with_security_headers(serve_inner(shared, req))
}

fn serve_inner(shared: &Shared, req: &Request) -> Response {
    let head = &req.head;
    let ip = canonical(req.peer);
    let now = Instant::now();
    let (host_validation, csrf, domains) = {
        let mut st = shared.lock();
        if st.bans.is_banned(ip, now) {
            return Response::text(
                403,
                "Your IP address has been banned after too many failed authentication attempts.",
            );
        }
        (st.prefs.host_header_validation, st.prefs.csrf_protection, st.prefs.domain_list.clone())
    };
    if host_validation && !host_allowed(head.header("host"), &domains) {
        return Response::text(400, "Bad Request: unexpected Host header");
    }
    let api = head.path.starts_with("/api/");
    if csrf && api && !csrf_ok(head) {
        return Response::text(401, "Unauthorized");
    }
    let read = matches!(head.method.as_str(), "GET" | "HEAD");
    if !read && head.method != "POST" {
        return Response::text(405, "Method Not Allowed").with_header("Allow", "GET, HEAD, POST");
    }
    match head.path.as_str() {
        "/" | "/index.html" => {
            return Response::new(200, "text/html; charset=UTF-8", PAGE).with_header(
                "Content-Security-Policy",
                "default-src 'self'; style-src 'self' 'unsafe-inline'; frame-ancestors 'self'; form-action 'self'",
            );
        }
        "/app.js" => return Response::new(200, "text/javascript; charset=UTF-8", SCRIPT),
        _ => {}
    }
    let Some(endpoint) = head.path.strip_prefix("/api/v2/") else {
        return Response::text(404, "Not Found");
    };
    let p = match params(req) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if endpoint == "auth/login" {
        if read {
            return Response::text(405, "Method Not Allowed").with_header("Allow", "POST");
        }
        return login(shared, &p, ip, now);
    }
    let Some(key) = authenticate(&mut shared.lock(), head, ip, now) else {
        return Response::text(403, "Forbidden");
    };
    // Endpoints that change something take POST only (405 otherwise).
    let post_only = matches!(
        endpoint,
        "auth/logout"
            | "app/setPreferences"
            | "app/shutdown"
            | "transfer/toggleSpeedLimitsMode"
            | "transfer/setSpeedLimitsMode"
            | "transfer/setDownloadLimit"
            | "transfer/setUploadLimit"
            | "torrents/add"
            | "torrents/stop"
            | "torrents/pause"
            | "torrents/start"
            | "torrents/resume"
            | "torrents/delete"
            | "torrents/recheck"
            | "torrents/reannounce"
            | "torrents/filePrio"
            | "torrents/renameFile"
            | "torrents/renameFolder"
            | "torrents/setDownloadLimit"
            | "torrents/setUploadLimit"
            | "torrents/setForceStart"
            | "torrents/setSuperSeeding"
            | "torrents/setCategory"
            | "torrents/createCategory"
            | "torrents/editCategory"
            | "torrents/removeCategories"
    );
    if post_only && read {
        return Response::text(405, "Method Not Allowed").with_header("Allow", "POST");
    }
    match endpoint {
        "auth/logout" => {
            shared.lock().sessions.remove(&key);
            Response::ok().with_header("Set-Cookie", "SID=; HttpOnly; SameSite=Strict; path=/; Max-Age=0")
        }
        "app/version" => Response::text(200, COMPAT_VERSION),
        "app/webapiVersion" => Response::text(200, WEBAPI_VERSION),
        "app/buildInfo" => Response::json(
            &Json::obj()
                .with("qt", "")
                .with("libtorrent", "2.0.11.0")
                .with("boost", "")
                .with("openssl", "")
                .with("zlib", "")
                .with("bitness", usize::BITS)
                .with("platform", std::env::consts::OS)
                .with("app", "PaTorrent")
                .with("app_version", VERSION),
        ),
        "app/preferences" => {
            let st = shared.lock();
            Response::json(&preferences(&st, shared))
        }
        "app/setPreferences" => {
            let Some(text) = p.get("json") else { return Response::text(400, "missing json") };
            let Ok(v @ Json::Obj(_)) = json::parse(text) else { return Response::text(400, "invalid json") };
            let mut st = shared.lock();
            apply_preferences(shared, &mut st, &v);
            shared.save(&st);
            Response::ok()
        }
        "app/defaultSavePath" => Response::text(200, shared.lock().prefs.save_path.clone()),
        "app/shutdown" => {
            shared.lock().shutdown = true;
            Response::ok()
        }
        "transfer/info" => {
            let st = shared.lock();
            Response::json(&Json::Obj(transfer_fields(&st, false)))
        }
        "transfer/speedLimitsMode" => {
            Response::text(200, if shared.lock().prefs.alt_enabled { "1" } else { "0" })
        }
        "transfer/toggleSpeedLimitsMode" | "transfer/setSpeedLimitsMode" => {
            let mut st = shared.lock();
            st.prefs.alt_enabled = if endpoint == "transfer/toggleSpeedLimitsMode" {
                !st.prefs.alt_enabled
            } else {
                match p.get("mode") {
                    Some("0") => false,
                    Some("1") => true,
                    _ => return Response::text(400, "mode must be 0 or 1"),
                }
            };
            shared.apply_limits(&mut st);
            Response::ok()
        }
        "transfer/downloadLimit" | "transfer/uploadLimit" => {
            let st = shared.lock();
            let v = if endpoint == "transfer/downloadLimit" {
                st.settings.download_rate_limit
            } else {
                st.settings.upload_rate_limit
            };
            Response::text(200, v.to_string())
        }
        "transfer/setDownloadLimit" | "transfer/setUploadLimit" => {
            let Some(limit) = p.int("limit") else { return Response::text(400, "missing limit") };
            let limit = limit.clamp(0, u32::MAX as i64) as u32;
            let mut st = shared.lock();
            let alt = st.prefs.alt_enabled;
            let slot = match (endpoint == "transfer/setDownloadLimit", alt) {
                (true, false) => &mut st.prefs.dl_limit,
                (true, true) => &mut st.prefs.alt_dl_limit,
                (false, false) => &mut st.prefs.up_limit,
                (false, true) => &mut st.prefs.alt_up_limit,
            };
            *slot = limit;
            shared.apply_limits(&mut st);
            Response::ok()
        }
        "sync/maindata" => maindata(shared, &key, p.int("rid").unwrap_or(0)),
        "torrents/info" => torrents_info(shared, &p),
        "torrents/count" => Response::text(200, shared.lock().torrents.len().to_string()),
        "torrents/properties" => properties(shared, &p),
        "torrents/trackers" => trackers(shared, &p),
        "torrents/files" => files(shared, &p),
        "torrents/pieceStates" => piece_states(shared, &p),
        "torrents/add" => add(shared, &p),
        "torrents/stop" | "torrents/pause" => for_each(shared, &p, |h| {
            h.unset_flags(TorrentFlags::AUTO_MANAGED);
            h.pause();
        }),
        "torrents/start" | "torrents/resume" => for_each(shared, &p, |h| {
            h.set_flags(TorrentFlags::AUTO_MANAGED);
            h.resume();
        }),
        "torrents/setForceStart" => {
            let force = p.flag("value");
            for_each(shared, &p, |h| {
                if force {
                    h.unset_flags(TorrentFlags::AUTO_MANAGED);
                    h.resume();
                } else {
                    h.set_flags(TorrentFlags::AUTO_MANAGED);
                }
            })
        }
        "torrents/setSuperSeeding" => {
            let on = p.flag("value");
            for_each(shared, &p, |h| {
                if on {
                    h.set_flags(TorrentFlags::SUPER_SEEDING)
                } else {
                    h.unset_flags(TorrentFlags::SUPER_SEEDING)
                }
            })
        }
        "torrents/recheck" => for_each(shared, &p, |h| h.force_recheck()),
        "torrents/reannounce" => for_each(shared, &p, |h| h.force_reannounce()),
        "torrents/delete" => {
            let mut flags = RemoveFlags::DELETE_PARTFILE;
            if p.flag("deleteFiles") {
                flags |= RemoveFlags::DELETE_FILES;
            }
            let ids = selected(&shared.lock(), &p);
            for id in ids {
                shared.handle.remove_torrent(id, flags);
            }
            Response::ok()
        }
        "torrents/filePrio" => file_prio(shared, &p),
        "torrents/renameFile" => rename(shared, &p, false),
        "torrents/renameFolder" => rename(shared, &p, true),
        "torrents/setDownloadLimit" | "torrents/setUploadLimit" => {
            let Some(limit) = p.int("limit") else { return Response::text(400, "missing limit") };
            let limit = limit.clamp(0, u32::MAX as i64) as u32;
            let down = endpoint == "torrents/setDownloadLimit";
            for_each(
                shared,
                &p,
                |h| if down { h.set_download_limit(limit) } else { h.set_upload_limit(limit) },
            )
        }
        "torrents/downloadLimit" | "torrents/uploadLimit" => {
            let st = shared.lock();
            let mut out = Json::obj();
            for id in selected(&st, &p) {
                let e = &st.torrents[&id];
                let v = if endpoint == "torrents/downloadLimit" {
                    e.status.download_limit
                } else {
                    e.status.upload_limit
                };
                out.set(&e.hash, v);
            }
            Response::json(&out)
        }
        "torrents/categories" => Response::json(&categories_json(&shared.lock().categories)),
        "torrents/createCategory" | "torrents/editCategory" => {
            let name = p.get("category").unwrap_or("").trim().to_owned();
            if name.is_empty() {
                return Response::text(400, "category name is empty");
            }
            let save_path = p.get("savePath").unwrap_or("").to_owned();
            let mut st = shared.lock();
            let exists = st.categories.contains_key(&name);
            if endpoint == "torrents/createCategory" {
                if !valid_category(&name) || exists {
                    return Response::text(409, "invalid category name");
                }
            } else if !exists {
                return Response::text(409, "no such category");
            }
            st.categories.insert(name, save_path);
            st.categories_version += 1;
            shared.save(&st);
            Response::ok()
        }
        "torrents/removeCategories" => {
            let mut st = shared.lock();
            let names: Vec<String> =
                p.get("categories").unwrap_or("").split('\n').map(|s| s.trim().to_owned()).collect();
            for n in &names {
                st.categories.remove(n);
            }
            let cleared: Vec<String> = st
                .torrent_categories
                .iter()
                .filter(|(_, c)| names.contains(c))
                .map(|(h, _)| h.clone())
                .collect();
            for h in cleared {
                st.torrent_categories.remove(&h);
                st.touch(&h);
            }
            st.categories_version += 1;
            shared.save(&st);
            Response::ok()
        }
        "torrents/setCategory" => {
            let cat = p.get("category").unwrap_or("").trim().to_owned();
            let mut st = shared.lock();
            if !cat.is_empty() && !st.categories.contains_key(&cat) {
                return Response::text(409, "category does not exist");
            }
            for id in selected(&st, &p) {
                let hash = st.torrents[&id].hash.clone();
                if cat.is_empty() {
                    st.torrent_categories.remove(&hash);
                } else {
                    st.torrent_categories.insert(hash.clone(), cat.clone());
                }
                st.touch(&hash);
            }
            shared.save(&st);
            Response::ok()
        }
        "torrents/tags" => Response::json(&Json::Arr(Vec::new())),
        _ => Response::text(404, "Not Found"),
    }
}

fn login(shared: &Shared, p: &Params, ip: IpAddr, now: Instant) -> Response {
    let user = p.get("username").unwrap_or("");
    let pass = p.get("password").unwrap_or("");
    let (expected_user, hash) = {
        let st = shared.lock();
        (st.username.clone(), st.password.clone())
    };
    // The hash is checked even for a wrong user name, so the time taken does not tell.
    let pass_ok = hash.verify(pass);
    let ok = constant_time_eq(user.as_bytes(), expected_user.as_bytes()) & pass_ok;
    let mut st = shared.lock();
    if !ok {
        let (max, ban) = (st.prefs.max_auth_fail_count, Duration::from_secs(st.prefs.ban_duration));
        st.bans.failed(ip, max, ban, now);
        return Response::text(200, "Fails.");
    }
    st.bans.succeeded(ip);
    if st.sessions.len() >= MAX_SESSIONS {
        if let Some(oldest) = st.sessions.iter().min_by_key(|(_, s)| s.last_used).map(|(k, _)| k.clone()) {
            st.sessions.remove(&oldest);
        }
    }
    let sid = random_hex(16);
    st.sessions.insert(sid.clone(), ClientSession { last_used: now, maindata: None });
    Response::text(200, "Ok.")
        .with_header("Set-Cookie", format!("SID={sid}; HttpOnly; SameSite=Strict; path=/"))
}

fn valid_category(name: &str) -> bool {
    !name.starts_with('/')
        && !name.ends_with('/')
        && !name.contains("//")
        && !name.contains('\\')
        && name.chars().all(|c| !c.is_control())
}

fn categories_json(cats: &BTreeMap<String, String>) -> Json {
    let mut out = Json::obj();
    for (name, path) in cats {
        out.set(name, Json::obj().with("name", name.as_str()).with("savePath", path.as_str()));
    }
    out
}

/// Torrents named by `hashes` (`|`-separated, or `all`), in no particular order.
fn selected(st: &State, p: &Params) -> Vec<TorrentId> {
    let hashes = p.get("hashes").unwrap_or("");
    if hashes == "all" {
        return st.torrents.keys().copied().collect();
    }
    hashes.split('|').filter_map(|h| st.by_hash.get(&h.trim().to_ascii_lowercase()).copied()).collect()
}

fn for_each(shared: &Shared, p: &Params, f: impl Fn(&patorrent_core::TorrentHandle)) -> Response {
    let ids = selected(&shared.lock(), p);
    for id in ids {
        f(&shared.handle.torrent(id));
    }
    Response::ok()
}

fn one(st: &State, p: &Params) -> Option<TorrentId> {
    st.by_hash.get(&p.get("hash")?.trim().to_ascii_lowercase()).copied()
}

fn extra<'a>(st: &'a State, hash: &str, last_activity: i64) -> Extra<'a> {
    Extra {
        category: st.torrent_categories.get(hash).map_or("", String::as_str),
        last_activity,
        private: st.meta.get(hash).is_some_and(|m| m.private),
    }
}

fn kib_or_unlimited(bytes: u32) -> i64 {
    if bytes == 0 { -1 } else { (bytes / 1024) as i64 }
}

fn listen_port(s: &patorrent_core::Settings) -> i64 {
    s.listen_interfaces
        .split(',')
        .next()
        .and_then(|i| i.rsplit_once(':'))
        .and_then(|(_, p)| p.trim().parse().ok())
        .unwrap_or(0)
}

fn proxy_type_number(t: ProxyType) -> i64 {
    match t {
        ProxyType::None => -1,
        ProxyType::Http => 1,
        ProxyType::Socks5 => 2,
        ProxyType::HttpPw => 3,
        ProxyType::Socks5Pw => 4,
        ProxyType::Socks4 => 5,
    }
}

/// `app/preferences`: the subset we support.
fn preferences(st: &State, shared: &Shared) -> Json {
    let s = &st.settings;
    let p = &st.prefs;
    let _ = shared;
    Json::obj()
        .with("locale", "en")
        .with("save_path", p.save_path.as_str())
        .with("temp_path_enabled", false)
        .with("temp_path", "")
        .with("start_paused_enabled", p.start_paused)
        .with("listen_port", listen_port(s))
        .with("upnp", s.enable_upnp || s.enable_natpmp)
        .with("dl_limit", kib_or_unlimited(p.dl_limit))
        .with("up_limit", kib_or_unlimited(p.up_limit))
        .with("alt_dl_limit", kib_or_unlimited(p.alt_dl_limit))
        .with("alt_up_limit", kib_or_unlimited(p.alt_up_limit))
        .with("max_connec", s.connections_limit)
        .with("max_uploads", s.unchoke_slots_limit)
        .with("dht", s.enable_dht)
        .with("pex", true)
        .with("lsd", s.enable_lsd)
        .with(
            "encryption",
            match s.out_enc_policy {
                EncPolicy::Enabled => 0i64,
                EncPolicy::Forced => 1,
                EncPolicy::Disabled => 2,
            },
        )
        .with("queueing_enabled", true)
        .with("max_active_downloads", s.active_downloads)
        .with("max_active_torrents", s.active_limit)
        .with("max_active_uploads", s.active_seeds)
        .with("proxy_type", proxy_type_number(s.proxy_type))
        .with("proxy_ip", s.proxy_hostname.as_str())
        .with("proxy_port", s.proxy_port as i64)
        .with("proxy_auth_enabled", matches!(s.proxy_type, ProxyType::HttpPw | ProxyType::Socks5Pw))
        .with("proxy_username", s.proxy_username.as_str())
        .with("proxy_peer_connections", s.proxy_peer_connections)
        .with("web_ui_username", st.username.as_str())
        .with("bypass_local_auth", p.bypass_local_auth)
        .with("web_ui_csrf_protection_enabled", p.csrf_protection)
        .with("web_ui_host_header_validation_enabled", p.host_header_validation)
        .with("web_ui_domain_list", p.domain_list.as_str())
        .with("web_ui_max_auth_fail_count", p.max_auth_fail_count)
        .with("web_ui_ban_duration", p.ban_duration)
        .with("web_ui_session_timeout", p.session_timeout)
}

/// Keys never written to the state file (secrets; the password is stored as a hash instead).
const NOT_STORED: [&str; 3] = ["web_ui_password", "proxy_password", "web_ui_username"];

/// Applies the `setPreferences` keys we support (unknown keys are ignored) and remembers them
/// for the next start.
pub(crate) fn apply_preferences(shared: &Shared, st: &mut State, v: &Json) {
    let mut settings_changed = false;
    let mut limits_changed = false;
    let kib =
        |j: &Json| j.as_i64().map(|k| if k <= 0 { 0 } else { (k.min(u32::MAX as i64 / 1024) * 1024) as u32 });
    for (key, val) in v.entries() {
        let s = &mut st.settings;
        let accepted = match key.as_str() {
            "save_path" => val.as_str().map(|p| st.prefs.save_path = p.to_owned()).is_some(),
            "start_paused_enabled" => val.as_bool().map(|b| st.prefs.start_paused = b).is_some(),
            "dl_limit" => {
                kib(val).map(|b| st.prefs.dl_limit = b).is_some() && {
                    limits_changed = true;
                    true
                }
            }
            "up_limit" => {
                kib(val).map(|b| st.prefs.up_limit = b).is_some() && {
                    limits_changed = true;
                    true
                }
            }
            "alt_dl_limit" => {
                kib(val).map(|b| st.prefs.alt_dl_limit = b).is_some() && {
                    limits_changed = true;
                    true
                }
            }
            "alt_up_limit" => {
                kib(val).map(|b| st.prefs.alt_up_limit = b).is_some() && {
                    limits_changed = true;
                    true
                }
            }
            "listen_port" => match val.as_i64() {
                Some(port @ 0..=65535) => {
                    s.listen_interfaces = format!("0.0.0.0:{port},[::]:{port}");
                    true
                }
                _ => false,
            },
            "upnp" => val.as_bool().map(|b| (s.enable_upnp, s.enable_natpmp) = (b, b)).is_some(),
            "max_connec" => {
                val.as_i64().map(|n| s.connections_limit = n.clamp(-1, i32::MAX as i64) as i32).is_some()
            }
            "max_uploads" => {
                val.as_i64().map(|n| s.unchoke_slots_limit = n.clamp(-1, i32::MAX as i64) as i32).is_some()
            }
            "dht" => val.as_bool().map(|b| s.enable_dht = b).is_some(),
            "lsd" => val.as_bool().map(|b| s.enable_lsd = b).is_some(),
            "encryption" => match val.as_i64() {
                Some(0) => {
                    (s.out_enc_policy, s.in_enc_policy) = (EncPolicy::Enabled, EncPolicy::Enabled);
                    true
                }
                Some(1) => {
                    (s.out_enc_policy, s.in_enc_policy) = (EncPolicy::Forced, EncPolicy::Forced);
                    true
                }
                Some(2) => {
                    (s.out_enc_policy, s.in_enc_policy) = (EncPolicy::Disabled, EncPolicy::Disabled);
                    true
                }
                _ => false,
            },
            "max_active_downloads" => {
                val.as_i64().map(|n| s.active_downloads = n.clamp(-1, i32::MAX as i64) as i32).is_some()
            }
            "max_active_torrents" => {
                val.as_i64().map(|n| s.active_limit = n.clamp(-1, i32::MAX as i64) as i32).is_some()
            }
            "max_active_uploads" => {
                val.as_i64().map(|n| s.active_seeds = n.clamp(-1, i32::MAX as i64) as i32).is_some()
            }
            "proxy_type" => {
                let t = match val.as_i64() {
                    Some(-1 | 0) => Some(ProxyType::None),
                    Some(1) => Some(ProxyType::Http),
                    Some(2) => Some(ProxyType::Socks5),
                    Some(3) => Some(ProxyType::HttpPw),
                    Some(4) => Some(ProxyType::Socks5Pw),
                    Some(5) => Some(ProxyType::Socks4),
                    _ => None,
                };
                t.map(|t| s.proxy_type = t).is_some()
            }
            "proxy_ip" => val.as_str().map(|h| s.proxy_hostname = h.to_owned()).is_some(),
            "proxy_port" => match val.as_i64() {
                Some(port @ 0..=65535) => {
                    s.proxy_port = port as u16;
                    true
                }
                _ => false,
            },
            "proxy_username" => val.as_str().map(|u| s.proxy_username = u.to_owned()).is_some(),
            "proxy_password" => val.as_str().map(|u| s.proxy_password = u.to_owned()).is_some(),
            "proxy_peer_connections" => val.as_bool().map(|b| s.proxy_peer_connections = b).is_some(),
            "web_ui_username" => match val.as_str() {
                Some(u) if !u.is_empty() => {
                    st.username = u.to_owned();
                    true
                }
                _ => false,
            },
            "web_ui_password" => match val.as_str() {
                Some(pw) if !pw.is_empty() => {
                    st.password = PasswordHash::new(pw, shared.iterations);
                    true
                }
                _ => false,
            },
            "bypass_local_auth" => val.as_bool().map(|b| st.prefs.bypass_local_auth = b).is_some(),
            "web_ui_csrf_protection_enabled" => val.as_bool().map(|b| st.prefs.csrf_protection = b).is_some(),
            "web_ui_host_header_validation_enabled" => {
                val.as_bool().map(|b| st.prefs.host_header_validation = b).is_some()
            }
            "web_ui_domain_list" => val.as_str().map(|d| st.prefs.domain_list = d.to_owned()).is_some(),
            "web_ui_max_auth_fail_count" => {
                val.as_i64().map(|n| st.prefs.max_auth_fail_count = n.clamp(0, 1000) as u32).is_some()
            }
            "web_ui_ban_duration" => {
                val.as_i64().map(|n| st.prefs.ban_duration = n.clamp(1, 86_400 * 365) as u64).is_some()
            }
            "web_ui_session_timeout" => {
                val.as_i64().map(|n| st.prefs.session_timeout = n.clamp(1, 86_400 * 365) as u64).is_some()
            }
            _ => false,
        };
        let is_setting = !matches!(
            key.as_str(),
            "save_path"
                | "start_paused_enabled"
                | "dl_limit"
                | "up_limit"
                | "alt_dl_limit"
                | "alt_up_limit"
                | "web_ui_username"
                | "web_ui_password"
                | "bypass_local_auth"
                | "web_ui_csrf_protection_enabled"
                | "web_ui_host_header_validation_enabled"
                | "web_ui_domain_list"
                | "web_ui_max_auth_fail_count"
                | "web_ui_ban_duration"
                | "web_ui_session_timeout"
        );
        if accepted {
            settings_changed |= is_setting;
            if !NOT_STORED.contains(&key.as_str()) {
                st.stored_prefs.set(key, val.clone());
            }
        }
    }
    if limits_changed {
        // also sends any other changed setting
        shared.apply_limits(st);
    } else if settings_changed {
        shared.handle.apply_settings(st.settings.clone());
    }
}

fn transfer_fields(st: &State, sync: bool) -> Vec<(String, Json)> {
    let s = &st.stats;
    let mut v: Vec<(String, Json)> = vec![
        (
            "connection_status".into(),
            (if s.has_incoming_connections { "connected" } else { "firewalled" }).into(),
        ),
        ("dht_nodes".into(), s.dht_nodes.into()),
        ("dl_info_data".into(), s.recv_payload_bytes.into()),
        ("dl_info_speed".into(), s.download_rate.into()),
        ("dl_rate_limit".into(), st.settings.download_rate_limit.into()),
        ("up_info_data".into(), s.sent_payload_bytes.into()),
        ("up_info_speed".into(), s.upload_rate.into()),
        ("up_rate_limit".into(), st.settings.upload_rate_limit.into()),
    ];
    if sync {
        v.push(("queueing".into(), true.into()));
        v.push(("use_alt_speed_limits".into(), st.prefs.alt_enabled.into()));
        v.push(("refresh_interval".into(), 1500i64.into()));
        v.push(("total_peer_connections".into(), s.num_peers_connected.into()));
    }
    v
}

/// What one client was last sent by `sync/maindata`.
pub struct Snapshot {
    rid: i64,
    torrents: HashMap<String, (u64, Vec<Json>)>,
    categories_version: u64,
    categories: BTreeMap<String, String>,
    server_state: Vec<(String, Json)>,
}

fn maindata(shared: &Shared, key: &str, rid: i64) -> Response {
    let mut guard = shared.lock();
    let st = &mut *guard;
    let old = st.sessions.get_mut(key).and_then(|s| s.maindata.take());
    let full = old.as_ref().is_none_or(|o| rid == 0 || o.rid != rid);
    let new_rid = old.as_ref().map_or(0, |o| o.rid) + 1;
    let mut out = Json::obj().with("rid", new_rid);
    if full {
        out.set("full_update", true);
    }
    let mut old = if full { None } else { old };
    let mut torrents = Json::obj();
    let mut snap_torrents = HashMap::with_capacity(st.torrents.len());
    for e in st.torrents.values() {
        let prev = old.as_mut().and_then(|o| o.torrents.remove(&e.hash));
        if let Some((v, vals)) = prev {
            if v == e.version {
                snap_torrents.insert(e.hash.clone(), (v, vals));
                continue;
            }
            let now = torrent::fields(&e.status, &extra(st, &e.hash, e.last_activity));
            let mut diff = Json::obj();
            for ((name, new), old) in torrent::FIELDS.iter().zip(&now).zip(&vals) {
                if new != old {
                    diff.set(name, new.clone());
                }
            }
            if !diff.entries().is_empty() {
                torrents.set(&e.hash, diff);
            }
            snap_torrents.insert(e.hash.clone(), (e.version, now));
        } else {
            let now = torrent::fields(&e.status, &extra(st, &e.hash, e.last_activity));
            torrents.set(
                &e.hash,
                Json::Obj(torrent::FIELDS.iter().map(|f| f.to_string()).zip(now.iter().cloned()).collect()),
            );
            snap_torrents.insert(e.hash.clone(), (e.version, now));
        }
    }
    if full || !torrents.entries().is_empty() {
        out.set("torrents", torrents);
    }
    if let Some(o) = &old {
        let removed: Vec<Json> = o.torrents.keys().map(|h| Json::Str(h.clone())).collect();
        if !removed.is_empty() {
            out.set("torrents_removed", Json::Arr(removed));
        }
    }
    // categories
    match &old {
        Some(o) if o.categories_version == st.categories_version => {}
        Some(o) => {
            let mut changed = BTreeMap::new();
            for (n, p) in &st.categories {
                if o.categories.get(n) != Some(p) {
                    changed.insert(n.clone(), p.clone());
                }
            }
            if !changed.is_empty() {
                out.set("categories", categories_json(&changed));
            }
            let removed: Vec<Json> = o
                .categories
                .keys()
                .filter(|n| !st.categories.contains_key(*n))
                .map(|n| Json::Str(n.clone()))
                .collect();
            if !removed.is_empty() {
                out.set("categories_removed", Json::Arr(removed));
            }
        }
        None => {
            out.set("categories", categories_json(&st.categories));
            out.set("tags", Json::Arr(Vec::new()));
        }
    }
    let server_state = transfer_fields(st, true);
    match &old {
        Some(o) => {
            let changed: Vec<(String, Json)> =
                server_state.iter().filter(|kv| !o.server_state.contains(kv)).cloned().collect();
            if !changed.is_empty() {
                out.set("server_state", Json::Obj(changed));
            }
        }
        None => out.set("server_state", Json::Obj(server_state.clone())),
    }
    let snap = Snapshot {
        rid: new_rid,
        torrents: snap_torrents,
        categories_version: st.categories_version,
        categories: st.categories.clone(),
        server_state,
    };
    if let Some(s) = st.sessions.get_mut(key) {
        s.maindata = Some(snap);
    }
    Response::json(&out)
}

fn cmp_json(a: &Json, b: &Json) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => match (a, b) {
            (Json::Str(x), Json::Str(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
            (Json::Bool(x), Json::Bool(y)) => x.cmp(y),
            _ => Ordering::Equal,
        },
    }
}

fn torrents_info(shared: &Shared, p: &Params) -> Response {
    let st = shared.lock();
    let filter = p.get("filter").unwrap_or("all");
    let category = p.get("category");
    let wanted: Option<Vec<String>> =
        p.get("hashes").map(|h| h.split('|').map(|x| x.trim().to_ascii_lowercase()).collect());
    let mut rows: Vec<Vec<Json>> = st
        .torrents
        .values()
        .filter(|e| torrent::matches_filter(filter, &e.status))
        .filter(|e| wanted.as_ref().is_none_or(|w| w.contains(&e.hash)))
        .filter(|e| {
            category.is_none_or(|c| st.torrent_categories.get(&e.hash).map_or("", String::as_str) == c)
        })
        .map(|e| torrent::fields(&e.status, &extra(&st, &e.hash, e.last_activity)))
        .collect();
    drop(st);
    let sort = p.get("sort").and_then(|s| torrent::FIELDS.iter().position(|f| *f == s));
    let name = torrent::FIELDS.iter().position(|f| *f == "name").expect("name field");
    let by = sort.unwrap_or(name);
    rows.sort_by(|a, b| cmp_json(&a[by], &b[by]).then_with(|| cmp_json(&a[name], &b[name])));
    if p.flag("reverse") {
        rows.reverse();
    }
    let len = rows.len() as i64;
    let mut offset = p.int("offset").unwrap_or(0);
    if offset < 0 {
        offset = (len + offset).max(0);
    }
    let offset = offset.min(len) as usize;
    let limit = p.int("limit").filter(|&l| l > 0).map_or(usize::MAX, |l| l as usize);
    let list: Vec<Json> = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|vals| Json::Obj(torrent::FIELDS.iter().map(|f| f.to_string()).zip(vals).collect()))
        .collect();
    Response::json(&Json::Arr(list))
}

fn not_found() -> Response {
    Response::text(404, "Torrent hash was not found")
}

fn properties(shared: &Shared, p: &Params) -> Response {
    let st = shared.lock();
    let Some(id) = one(&st, p) else { return not_found() };
    let e = &st.torrents[&id];
    let s = &e.status;
    let meta = st.meta.get(&e.hash).cloned().unwrap_or_default();
    let avg = |bytes: u64| bytes.checked_div(s.active_duration).unwrap_or(0);
    let lim = |b: u32| if b == 0 { -1 } else { b as i64 };
    let or_m1 = |t: i64| if t > 0 { t } else { -1 };
    let v = Json::obj()
        .with("save_path", s.save_path.as_str())
        .with("creation_date", meta.creation_date.unwrap_or(-1))
        .with("piece_size", if s.has_metadata { s.piece_length as i64 } else { -1 })
        .with("comment", meta.comment.as_str())
        .with("total_wasted", s.total_failed_bytes + s.total_redundant_bytes)
        .with("total_uploaded", s.all_time_upload)
        .with("total_uploaded_session", s.total_payload_upload)
        .with("total_downloaded", s.all_time_download)
        .with("total_downloaded_session", s.total_payload_download)
        .with("up_limit", lim(s.upload_limit))
        .with("dl_limit", lim(s.download_limit))
        .with("time_elapsed", s.active_duration)
        .with("seeding_time", s.seeding_duration)
        .with("nb_connections", s.num_connections)
        .with("nb_connections_limit", -1i64)
        .with("share_ratio", torrent::ratio(s))
        .with("addition_date", s.added_time)
        .with("completion_date", or_m1(s.completed_time))
        .with("created_by", meta.created_by.as_str())
        .with("dl_speed_avg", avg(s.all_time_download))
        .with("dl_speed", s.download_payload_rate)
        .with("eta", s.eta().map_or(torrent::MAX_ETA, |e| (e as i64).min(torrent::MAX_ETA)))
        .with("last_seen", or_m1(s.last_seen_complete))
        .with("peers", s.num_peers.saturating_sub(s.num_seeds))
        .with("peers_total", s.list_peers)
        .with("pieces_have", s.num_pieces)
        .with("pieces_num", if s.has_metadata { s.pieces_total as i64 } else { -1 })
        .with("reannounce", s.next_announce.unwrap_or(0))
        .with("seeds", s.num_seeds)
        .with("seeds_total", s.list_seeds)
        .with("total_size", if s.has_metadata { s.total as i64 } else { -1 })
        .with("up_speed_avg", avg(s.all_time_upload))
        .with("up_speed", s.upload_payload_rate)
        .with("isPrivate", meta.private)
        .with("hash", e.hash.as_str())
        .with("name", s.name.as_str());
    Response::json(&v)
}

fn trackers(shared: &Shared, p: &Params) -> Response {
    let (id, dht, lsd, pex, private) = {
        let st = shared.lock();
        let Some(id) = one(&st, p) else { return not_found() };
        let e = &st.torrents[&id];
        let private = st.meta.get(&e.hash).is_some_and(|m| m.private);
        let f = e.status.flags;
        (
            id,
            st.settings.enable_dht && !private && !f.contains(TorrentFlags::DISABLE_DHT),
            st.settings.enable_lsd && !private && !f.contains(TorrentFlags::DISABLE_LSD),
            !private && !f.contains(TorrentFlags::DISABLE_PEX),
            private,
        )
    };
    let special = |name: &str, on: bool| {
        Json::obj()
            .with("url", name)
            .with("status", if on { 2i64 } else { 0 })
            .with("tier", -1i64)
            .with("num_peers", 0i64)
            .with("num_seeds", 0i64)
            .with("num_leeches", 0i64)
            .with("num_downloaded", 0i64)
            .with("msg", if private { "This torrent is private" } else { "" })
    };
    let mut out = vec![special("** [DHT] **", dht), special("** [PeX] **", pex), special("** [LSD] **", lsd)];
    let Some(Reply::Trackers(list)) =
        shared.query(id, ReplyKind::Trackers, |h| h.torrent(id).post_trackers())
    else {
        return not_found();
    };
    let m1 = |v: Option<u32>| v.map_or(-1, i64::from);
    for t in list {
        out.push(
            Json::obj()
                .with("url", t.url.as_str())
                .with(
                    "status",
                    match t.status {
                        TrackerStatus::NotContacted => 1i64,
                        TrackerStatus::Working => 2,
                        TrackerStatus::Updating => 3,
                        TrackerStatus::Error => 4,
                    },
                )
                .with("tier", t.tier)
                .with("num_peers", -1i64)
                .with("num_seeds", m1(t.scrape_complete))
                .with("num_leeches", m1(t.scrape_incomplete))
                .with("num_downloaded", m1(t.scrape_downloaded))
                .with("msg", t.message.as_str()),
        );
    }
    Response::json(&Json::Arr(out))
}

/// The API's file priority for an engine priority.
fn api_priority(p: DownloadPriority) -> i64 {
    match p.0 {
        0 => 0,
        6 => 6,
        7 => 7,
        _ => 1,
    }
}

fn files(shared: &Shared, p: &Params) -> Response {
    let (id, seeding) = {
        let st = shared.lock();
        let Some(id) = one(&st, p) else { return not_found() };
        (id, st.torrents[&id].status.is_seeding)
    };
    let Some(Reply::Files(list)) = shared.query(id, ReplyKind::Files, |h| h.torrent(id).post_file_list())
    else {
        return not_found();
    };
    let done = match shared.query(id, ReplyKind::Progress, |h| h.torrent(id).post_file_progress()) {
        Some(Reply::Progress(d)) => d,
        _ => Vec::new(),
    };
    let avail = match shared.query(id, ReplyKind::Pieces, |h| h.torrent(id).post_piece_info()) {
        Some(Reply::Pieces(_, a)) => a,
        _ => Vec::new(),
    };
    let wanted: Option<Vec<usize>> =
        p.get("indexes").map(|s| s.split('|').filter_map(|i| i.trim().parse().ok()).collect());
    let mut out = Vec::new();
    for (i, f) in list.iter().enumerate() {
        if f.pad_file || wanted.as_ref().is_some_and(|w| !w.contains(&i)) {
            continue;
        }
        let d = done.get(i).copied().unwrap_or(0);
        let progress = if f.size == 0 { 1.0 } else { d as f64 / f.size as f64 };
        let pieces = avail.get(f.first_piece as usize..=f.last_piece as usize).unwrap_or(&[]);
        let availability = if pieces.is_empty() {
            -1.0
        } else {
            pieces.iter().map(|&a| a as f64).sum::<f64>() / pieces.len() as f64
        };
        out.push(
            Json::obj()
                .with("index", i)
                .with("name", f.path.as_str())
                .with("size", f.size)
                .with("progress", progress)
                .with("priority", api_priority(f.priority))
                .with("is_seed", seeding)
                .with("piece_range", Json::Arr(vec![f.first_piece.into(), f.last_piece.into()]))
                .with("availability", availability),
        );
    }
    Response::json(&Json::Arr(out))
}

fn piece_states(shared: &Shared, p: &Params) -> Response {
    let Some(id) = one(&shared.lock(), p) else { return not_found() };
    let Some(Reply::Pieces(have, _)) =
        shared.query(id, ReplyKind::Pieces, |h| h.torrent(id).post_piece_info())
    else {
        return not_found();
    };
    let v: Vec<Json> = (0..have.len()).map(|i| Json::Int(if have.get(i) { 2 } else { 0 })).collect();
    Response::json(&Json::Arr(v))
}

fn file_prio(shared: &Shared, p: &Params) -> Response {
    let prio = match p.int("priority") {
        Some(0) => DownloadPriority::DONT_DOWNLOAD,
        Some(1) => DownloadPriority::NORMAL,
        Some(6) => DownloadPriority(6),
        Some(7) => DownloadPriority::TOP,
        _ => return Response::text(400, "invalid priority"),
    };
    let mut ids = Vec::new();
    for s in p.get("id").unwrap_or("").split('|') {
        match s.trim().parse::<u32>() {
            Ok(i) => ids.push(i),
            Err(_) => return Response::text(400, "file id is not an integer"),
        }
    }
    let Some(id) = one(&shared.lock(), p) else { return not_found() };
    let files = match shared.query(id, ReplyKind::Files, |h| h.torrent(id).post_file_list()) {
        Some(Reply::Files(f)) if !f.is_empty() => f,
        _ => return Response::text(409, "torrent metadata hasn't downloaded yet"),
    };
    if ids.iter().any(|&i| i as usize >= files.len()) {
        return Response::text(409, "file id not found");
    }
    let h = shared.handle.torrent(id);
    for i in ids {
        h.set_file_priority(i, prio);
    }
    Response::ok()
}

/// A file path in the API's form: `/` separators, no leading or trailing ones.
fn api_path(p: &str) -> String {
    p.replace('\\', "/").trim_matches('/').to_owned()
}

/// qBittorrent's `Path::isValid` for a new name: relative, no empty, `.` or `..` parts, no
/// characters Windows forbids.
fn valid_new_path(p: &str) -> bool {
    !p.is_empty()
        && !p.contains(':')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
        && !p.chars().any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*') || c.is_control())
}

/// `torrents/renameFile` (`oldPath` names one file) and `torrents/renameFolder` (every file
/// under `oldPath`); paths as `torrents/files` names them, torrent folder included. The rename
/// runs in the background, as in qBittorrent.
fn rename(shared: &Shared, p: &Params, folder: bool) -> Response {
    let (Some(old), Some(new)) = (p.get("oldPath"), p.get("newPath")) else {
        return Response::text(400, "missing oldPath or newPath");
    };
    let absolute = new.starts_with(['/', '\\']);
    let (old, new) = (api_path(old), api_path(new));
    if absolute || !valid_new_path(&new) {
        return Response::text(409, "invalid newPath");
    }
    let Some(id) = one(&shared.lock(), p) else { return not_found() };
    let files = match shared.query(id, ReplyKind::Files, |h| h.torrent(id).post_file_list()) {
        Some(Reply::Files(f)) if !f.is_empty() => f,
        _ => return Response::text(409, "torrent metadata hasn't downloaded yet"),
    };
    let paths: Vec<String> = files.iter().map(|f| api_path(&f.path)).collect();
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
        return Response::text(409, if folder { "no such folder" } else { "no such file" });
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
        return Response::text(409, "newPath already in use");
    }
    let h = shared.handle.torrent(id);
    for (i, name) in todo {
        h.rename_file(i, name);
    }
    Response::ok()
}

/// A bare info-hash (40 hex or 32 base32 characters) as a magnet link.
fn bare_hash_magnet(s: &str) -> Option<String> {
    let hex = s.len() == 40 && s.bytes().all(|c| c.is_ascii_hexdigit());
    let b32 = s.len() == 32 && s.bytes().all(|c| c.is_ascii_alphabetic() || (b'2'..=b'7').contains(&c));
    (hex || b32).then(|| format!("magnet:?xt=urn:btih:{s}"))
}

fn add(shared: &Shared, p: &Params) -> Response {
    let mut sources: Vec<(TorrentSource, String, Option<Meta>)> = Vec::new();
    let mut invalid_file = false;
    let mut failures = 0;
    for url in p.get("urls").unwrap_or("").split('\n').map(str::trim).filter(|u| !u.is_empty()) {
        let magnet = if url.starts_with("magnet:") { Some(url.to_owned()) } else { bare_hash_magnet(url) };
        if let Some(m) = magnet {
            match parse_magnet_uri(&m) {
                Ok(parsed) => {
                    sources.push((TorrentSource::Magnet(m), torrent::hash_of(&parsed.info_hashes), None))
                }
                Err(_) => failures += 1,
            }
            continue;
        }
        match fetch::fetch(url) {
            Ok(fetch::Fetched::Torrent(bytes)) => match TorrentInfo::from_bytes(&bytes) {
                Ok(ti) => {
                    let meta = meta_of(&ti);
                    sources.push((
                        TorrentSource::TorrentFile(bytes),
                        torrent::hash_of(&ti.info_hashes()),
                        Some(meta),
                    ))
                }
                Err(_) => failures += 1,
            },
            Ok(fetch::Fetched::Magnet(m)) => match parse_magnet_uri(&m) {
                Ok(parsed) => {
                    sources.push((TorrentSource::Magnet(m), torrent::hash_of(&parsed.info_hashes), None))
                }
                Err(_) => failures += 1,
            },
            Err(_) => failures += 1,
        }
    }
    for f in &p.files {
        match TorrentInfo::from_bytes(&f.data) {
            Ok(ti) => {
                let meta = meta_of(&ti);
                sources.push((
                    TorrentSource::TorrentFile(f.data.clone()),
                    torrent::hash_of(&ti.info_hashes()),
                    Some(meta),
                ))
            }
            Err(_) => invalid_file = true,
        }
    }
    if sources.is_empty() {
        return if invalid_file {
            Response::text(415, "Torrent file is not valid.")
        } else {
            let _ = failures;
            Response::text(200, "Fails.")
        };
    }
    let mut st = shared.lock();
    let save_path =
        p.get("savepath").filter(|s| !s.trim().is_empty()).map_or(st.prefs.save_path.clone(), str::to_owned);
    let category = p.get("category").map(str::trim).unwrap_or("").to_owned();
    if !category.is_empty() && !st.categories.contains_key(&category) && valid_category(&category) {
        st.categories.insert(category.clone(), String::new());
        st.categories_version += 1;
    }
    let category_ok = !category.is_empty() && st.categories.contains_key(&category);
    let paused = match (p.get("stopped").or(p.get("paused")), st.prefs.start_paused) {
        (Some(v), _) => v.eq_ignore_ascii_case("true"),
        (None, d) => d,
    };
    let mut flags = TorrentFlags::APPLY_IP_FILTER | TorrentFlags::NEED_SAVE_RESUME;
    flags |= if paused { TorrentFlags::PAUSED } else { TorrentFlags::AUTO_MANAGED };
    if p.flag("skip_checking") {
        flags |= TorrentFlags::SEED_MODE;
    }
    if p.flag("sequentialDownload") {
        flags |= TorrentFlags::SEQUENTIAL_DOWNLOAD;
    }
    let lim = |name: &str| p.int(name).map_or(0, |v| v.clamp(0, u32::MAX as i64) as u32);
    for (source, hash, meta) in sources {
        let mut params = AddTorrentParams::new(source, save_path.clone());
        params.flags = flags;
        params.name = p.get("rename").filter(|r| !r.is_empty()).map(str::to_owned);
        params.download_limit = lim("dlLimit");
        params.upload_limit = lim("upLimit");
        if category_ok {
            st.torrent_categories.insert(hash.clone(), category.clone());
            st.touch(&hash);
        }
        if let Some(m) = meta {
            st.meta.insert(hash, m);
        }
        shared.handle.add_torrent(params);
    }
    shared.save(&st);
    // Ask for the new torrents' status now rather than at the next refresh.
    shared.handle.post_torrent_updates();
    Response::text(200, "Ok.")
}

fn meta_of(ti: &TorrentInfo) -> Meta {
    Meta {
        comment: ti.comment().to_owned(),
        created_by: ti.creator().to_owned(),
        creation_date: ti.creation_date(),
        private: ti.is_private(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head_with(headers: &[(&str, &str)]) -> Head {
        Head {
            method: "POST".into(),
            path: "/api/v2/x".into(),
            minor: 1,
            headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
            ..Head::default()
        }
    }

    #[test]
    fn host_validation() {
        assert!(host_allowed(Some("127.0.0.1:8080"), ""));
        assert!(host_allowed(Some("[::1]:8080"), ""));
        assert!(host_allowed(Some("LocalHost:1"), ""));
        assert!(host_allowed(None, ""));
        assert!(!host_allowed(Some("evil.example:8080"), ""));
        assert!(!host_allowed(Some("evil.example"), "nas.lan"));
        assert!(host_allowed(Some("nas.lan:8080"), "nas.lan"));
        assert!(host_allowed(Some("a.b.example"), "x;*.example"));
        assert!(!host_allowed(Some("example"), "*.example"));
        assert!(host_allowed(Some("anything"), "*"));
    }

    #[test]
    fn csrf() {
        assert!(csrf_ok(&head_with(&[("host", "localhost:8080")])));
        assert!(csrf_ok(&head_with(&[("host", "localhost:8080"), ("origin", "http://localhost:8080")])));
        assert!(csrf_ok(&head_with(&[("host", "LOCALHOST:8080"), ("referer", "http://localhost:8080/x?y")])));
        assert!(csrf_ok(&head_with(&[("host", "nas"), ("origin", "http://nas:80")])));
        assert!(!csrf_ok(&head_with(&[("host", "localhost:8080"), ("origin", "http://evil:8080")])));
        assert!(!csrf_ok(&head_with(&[("host", "localhost:8080"), ("origin", "null")])));
        assert!(!csrf_ok(&head_with(&[("host", "localhost:8080"), ("referer", "http://localhost:8081/")])));
        // Origin wins over Referer
        assert!(!csrf_ok(&head_with(&[
            ("host", "localhost:8080"),
            ("origin", "http://evil"),
            ("referer", "http://localhost:8080/")
        ])));
    }

    #[test]
    fn wildcards_and_hashes() {
        assert!(wildcard_match("*.lan", "nas.lan"));
        assert!(wildcard_match("n?s.*", "nas.lan"));
        assert!(!wildcard_match("*.lan", "nas.lan.evil"));
        assert_eq!(
            bare_hash_magnet(&"a".repeat(40)),
            Some(format!("magnet:?xt=urn:btih:{}", "a".repeat(40)))
        );
        assert_eq!(bare_hash_magnet("xyz"), None);
        assert!(valid_category("tv/hd") && !valid_category("/tv") && !valid_category("a//b"));
    }
}
