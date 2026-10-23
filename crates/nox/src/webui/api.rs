//! Request checks (Host, CSRF, login) and the WebAPI v2 endpoints.
//!
//! Written from qBittorrent's published WebAPI documentation ("WebUI API (qBittorrent 5.0)"):
//! names, parameters, units and status codes follow it. Where it is silent the choice is ours
//! and noted in CHANGELOG.md.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use patorrent_core::crypto::Sha1;
use patorrent_core::{
    DownloadPriority, EncPolicy, PeerFlags, PeerInfo, ProxyType, QueueMove, TorrentFlags, TorrentId,
    TrackerStatus, VERSION,
};
use patorrent_frontend::detail::peer_flags;
use patorrent_frontend::{icons, lang};

use super::auth::{constant_time_eq, random_hex};
use super::commands::{self, AddOptions, Source};
use super::http::{
    Head, Part, Response, cookie, multipart_boundary, parse_header_params, parse_multipart, parse_urlencoded,
};
use super::json::{self, Json};
use super::torrent::{self, Extra};
use super::{ClientSession, MAX_SESSIONS, Reply, ReplyKind, Shared, State, prefs};

/// The qBittorrent release whose WebAPI (2.11.2) this implements; clients pick features by it.
pub const COMPAT_VERSION: &str = "v5.0.0";
pub const WEBAPI_VERSION: &str = "2.11.2";
/// Largest body accepted before a client has logged in (the login form).
pub const UNAUTHENTICATED_BODY: usize = 64 * 1024;

/// The Web UI page: three static files and the favicon (the application icon's 32 px PNG,
/// `frontend::icons::app_png`) embedded in the binary (docs/WEBUI.md goal 3) and the
/// string table in the server's language (`lang.js`, made again when the language changes),
/// served with an `ETag` (a SHA-1 of the body) so a reload after an upgrade fetches them once
/// and a `Cache-Control: no-cache` so the browser revalidates every time.
pub struct StaticFile {
    pub path: &'static str,
    pub content_type: &'static str,
    pub body: &'static [u8],
    etag: String,
}

impl StaticFile {
    fn new(path: &'static str, content_type: &'static str, body: &'static [u8]) -> StaticFile {
        StaticFile { path, content_type, body, etag: format!("\"{}\"", Sha1::digest(body).to_hex()) }
    }
}

static STATIC_FILES: LazyLock<[StaticFile; 4]> = LazyLock::new(|| {
    [
        StaticFile::new("/", "text/html; charset=UTF-8", include_bytes!("page.html")),
        StaticFile::new("/style.css", "text/css; charset=UTF-8", include_bytes!("style.css")),
        StaticFile::new("/app.js", "text/javascript; charset=UTF-8", include_bytes!("app.js")),
        StaticFile::new("/favicon.png", "image/png", icons::app_png(32).unwrap_or_default()),
    ]
});

/// `lang.js` for the current language: made at the first request and again after a switch
/// (`lang::generation`); each one is leaked, as language switches are rare.
static LANG_FILE: Mutex<Option<(usize, &'static StaticFile)>> = Mutex::new(None);

fn lang_file() -> &'static StaticFile {
    let mut cached = LANG_FILE.lock().unwrap_or_else(PoisonError::into_inner);
    let generation = lang::generation();
    match *cached {
        Some((g, f)) if g == generation => f,
        _ => {
            let body: &'static [u8] = Box::leak(lang_js().into_boxed_str()).as_bytes();
            let f: &'static StaticFile =
                Box::leak(Box::new(StaticFile::new("/lang.js", "text/javascript; charset=UTF-8", body)));
            *cached = Some((generation, f));
            f
        }
    }
}

/// `window.PA_LANG` for the page (`lang.js`, loaded before `app.js`): the language's code and
/// every text of the string table by key, without the `&` mnemonics (D20, `frontend::lang`).
pub fn lang_js() -> String {
    let mut texts = Json::obj();
    for (key, text) in lang::entries() {
        texts.set(key, lang::plain(text));
    }
    let dir = if lang::right_to_left() { "rtl" } else { "ltr" };
    let table = Json::obj().with("code", lang::current()).with("dir", dir).with("t", texts);
    format!("window.PA_LANG = {};\n", table.to_json())
}

/// The page sets `default-src 'self'`: no inline script or style, nothing from elsewhere.
const PAGE_CSP: &str = "default-src 'self'; frame-ancestors 'self'; form-action 'self'";

pub fn static_file(path: &str) -> Option<&'static StaticFile> {
    let path = if path == "/index.html" { "/" } else { path };
    if path == "/lang.js" {
        return Some(lang_file());
    }
    STATIC_FILES.iter().find(|f| f.path == path)
}

/// A static file, or `304 Not Modified` when the client's `If-None-Match` names its ETag.
fn serve_static(f: &'static StaticFile, head: &Head) -> Response {
    let matches = head.header("if-none-match").is_some_and(|v| {
        v.split(',').any(|t| t.trim().trim_start_matches("W/") == f.etag || t.trim() == "*")
    });
    let r = if matches { Response::new(304, "", "") } else { Response::new(200, f.content_type, f.body) };
    let r = r.with_header("ETag", f.etag.clone()).with_header("Cache-Control", "no-cache");
    if f.path == "/" { r.with_header("Content-Security-Policy", PAGE_CSP) } else { r }
}

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
            .or_insert(ClientSession { last_used: now, maindata: None, peers: None })
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

/// The headers every response carries (docs/WEBUI.md "Security"), unless the response set
/// its own (the static files have their own `Cache-Control`).
pub(super) fn with_security_headers(mut r: Response) -> Response {
    for (name, value) in [
        ("X-Frame-Options", "SAMEORIGIN"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Cache-Control", "no-store"),
    ] {
        if !r.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
            r = r.with_header(name, value);
        }
    }
    r
}

/// The checks a `GET /pa/ws` upgrade must pass before the socket is switched: the ban list,
/// the `Host` header, `Origin` (the CSRF rule of [`csrf_ok`], refused with 403 here since a
/// WebSocket is cross-origin by default in browsers), a valid session exactly like a request
/// ([`authenticate`]), and the upgrade headers themselves. Returns the session key and the
/// client's `Sec-WebSocket-Key`.
pub(super) fn websocket_handshake(
    shared: &Shared,
    head: &Head,
    ip: IpAddr,
) -> Result<(String, String), Response> {
    let ip = canonical(ip);
    let now = Instant::now();
    let mut st = shared.lock();
    if st.bans.is_banned(ip, now) {
        return Err(Response::text(
            403,
            "Your IP address has been banned after too many failed authentication attempts.",
        ));
    }
    if st.prefs.host_header_validation && !host_allowed(head.header("host"), &st.prefs.domain_list) {
        return Err(Response::text(400, "Bad Request: unexpected Host header"));
    }
    if st.prefs.csrf_protection && !csrf_ok(head) {
        return Err(Response::text(403, "Forbidden: origin not allowed"));
    }
    let Some(key) = authenticate(&mut st, head, ip, now) else {
        return Err(Response::text(403, "Forbidden"));
    };
    drop(st);
    match super::ws::handshake_key(head) {
        Ok(ws_key) => Ok((key, ws_key.to_owned())),
        Err(e) => {
            let r = Response::text(e.status(), e.message());
            Err(if e.status() == 426 { r.with_header("Sec-WebSocket-Version", "13") } else { r })
        }
    }
}

fn json_error(status: u16, msg: &str) -> Response {
    Response::new(status, "application/json", Json::obj().with("error", msg).to_json())
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
    let native = head.path.starts_with("/pa/");
    if csrf && native && !csrf_ok(head) {
        return json_error(403, "origin not allowed");
    }
    let read = matches!(head.method.as_str(), "GET" | "HEAD");
    if !read && head.method != "POST" {
        return Response::text(405, "Method Not Allowed").with_header("Allow", "GET, HEAD, POST");
    }
    if let Some(f) = static_file(&head.path) {
        if !read {
            return Response::text(405, "Method Not Allowed").with_header("Allow", "GET, HEAD");
        }
        return serve_static(f, head);
    }
    if let Some(endpoint) = head.path.strip_prefix("/pa/") {
        // the native commands (docs/WEBUI.md "Commands"): JSON in, 204 or {"error"} out
        if authenticate(&mut shared.lock(), head, ip, now).is_none() {
            return json_error(403, "forbidden");
        }
        return commands::serve(shared, endpoint, req);
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
            | "torrents/setLocation"
            | "torrents/editTracker"
            | "torrents/addTrackers"
            | "torrents/removeTrackers"
            | "torrents/addPeers"
            | "torrents/setShareLimits"
            | "torrents/topPrio"
            | "torrents/bottomPrio"
            | "torrents/increasePrio"
            | "torrents/decreasePrio"
    );
    if post_only && read {
        return Response::text(405, "Method Not Allowed").with_header("Allow", "POST");
    }
    match endpoint {
        "auth/logout" => {
            shared.lock().sessions.remove(&key);
            Response::ok()
                .with_header("Set-Cookie", format!("SID=; {}; Max-Age=0", cookie_attributes(shared)))
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
        "sync/torrentPeers" => torrent_peers(shared, &key, &p),
        "torrents/info" => torrents_info(shared, &p),
        "torrents/count" => Response::text(200, shared.lock().torrents.len().to_string()),
        "torrents/properties" => properties(shared, &p),
        "torrents/trackers" => trackers(shared, &p),
        "torrents/files" => files(shared, &p),
        "torrents/pieceStates" => piece_states(shared, &p),
        "torrents/add" => add(shared, &p),
        // the torrent commands share their implementation with the native API (commands.rs);
        // qBittorrent's stop and pause are both µTorrent's Stop
        "torrents/stop" | "torrents/pause" => {
            // (the guard must be gone before the command takes the lock itself)
            let ids = selected(&shared.lock(), &p);
            commands::stop(shared, &ids);
            Response::ok()
        }
        "torrents/start" | "torrents/resume" => {
            let ids = selected(&shared.lock(), &p);
            commands::start(shared, &ids);
            Response::ok()
        }
        "torrents/setForceStart" => {
            let ids = selected(&shared.lock(), &p);
            if p.flag("value") {
                commands::force_start(shared, &ids);
            } else {
                commands::unforce(shared, &ids);
            }
            Response::ok()
        }
        "torrents/setSuperSeeding" => {
            commands::super_seed(shared, &selected(&shared.lock(), &p), p.flag("value"));
            Response::ok()
        }
        "torrents/recheck" => {
            commands::recheck(shared, &selected(&shared.lock(), &p));
            Response::ok()
        }
        "torrents/reannounce" => {
            commands::reannounce(shared, &selected(&shared.lock(), &p));
            Response::ok()
        }
        "torrents/delete" => {
            commands::remove(shared, &selected(&shared.lock(), &p), p.flag("deleteFiles"));
            Response::ok()
        }
        "torrents/setLocation" => set_location(shared, &p),
        "torrents/editTracker" => {
            let Some(id) = one(&shared.lock(), &p) else { return not_found() };
            let (Some(orig), Some(new)) = (p.get("origUrl"), p.get("newUrl")) else {
                return Response::text(400, "origUrl and newUrl are required");
            };
            match commands::edit_tracker(shared, id, orig, new) {
                Ok(()) => Response::ok(),
                Err(e) => Response::text(e.status, e.msg),
            }
        }
        "torrents/addTrackers" => {
            let Some(id) = one(&shared.lock(), &p) else { return not_found() };
            commands::add_trackers(shared, &[id], p.get("urls").unwrap_or(""));
            Response::ok()
        }
        "torrents/addPeers" => add_peers(shared, &p),
        "torrents/removeTrackers" => {
            // `hash` may name several torrents (`|`-separated, or `all`)
            let ids = selected(
                &shared.lock(),
                &Params {
                    pairs: vec![("hashes".into(), p.get("hash").unwrap_or("").into())],
                    files: Vec::new(),
                },
            );
            if ids.is_empty() {
                return not_found();
            }
            match commands::remove_trackers(shared, &ids, p.get("urls").unwrap_or("")) {
                Ok(()) => Response::ok(),
                Err(e) => Response::text(e.status, e.msg),
            }
        }
        "torrents/setShareLimits" => {
            let ratio = p.get("ratioLimit").and_then(|v| v.trim().parse::<f64>().ok());
            let (Some(ratio), Some(minutes)) = (ratio, p.int("seedingTimeLimit")) else {
                return Response::text(400, "ratioLimit and seedingTimeLimit are required");
            };
            let ids = selected(&shared.lock(), &p); // (the guard goes before share_limits locks)
            commands::share_limits(shared, &ids, ratio, minutes);
            Response::ok()
        }
        "torrents/topPrio" | "torrents/bottomPrio" | "torrents/increasePrio" | "torrents/decreasePrio" => {
            let m = match endpoint {
                "torrents/topPrio" => QueueMove::Top,
                "torrents/bottomPrio" => QueueMove::Bottom,
                "torrents/increasePrio" => QueueMove::Up,
                _ => QueueMove::Down,
            };
            let ids = selected(&shared.lock(), &p); // (the guard goes before queue locks)
            commands::queue(shared, &ids, m);
            Response::ok()
        }
        "torrents/filePrio" => file_prio(shared, &p),
        "torrents/renameFile" => rename(shared, &p, false),
        "torrents/renameFolder" => rename(shared, &p, true),
        "torrents/setDownloadLimit" | "torrents/setUploadLimit" => {
            let Some(limit) = p.int("limit") else { return Response::text(400, "missing limit") };
            let limit = Some(limit.clamp(0, u32::MAX as i64) as u32);
            let ids = selected(&shared.lock(), &p);
            if endpoint == "torrents/setDownloadLimit" {
                commands::limits(shared, &ids, limit, None);
            } else {
                commands::limits(shared, &ids, None, limit);
            }
            Response::ok()
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
            let ids = {
                let st = shared.lock();
                if !cat.is_empty() && !st.categories.contains_key(&cat) {
                    return Response::text(409, "category does not exist");
                }
                selected(&st, &p)
            };
            match commands::label(shared, &ids, &cat) {
                Ok(()) => Response::ok(),
                Err(e) => Response::text(e.status, e.msg),
            }
        }
        "torrents/tags" => Response::json(&Json::Arr(Vec::new())),
        e if e.starts_with("rss/") => {
            let e = &e["rss/".len()..];
            if read && super::rss::shim_post_only(e) {
                return Response::text(405, "Method Not Allowed").with_header("Allow", "POST");
            }
            super::rss::shim(shared, e, &|k| p.get(k).map(str::to_owned))
        }
        _ => Response::text(404, "Not Found"),
    }
}

/// Checks a user name and password (the hash outside the state lock: it is slow on purpose),
/// counting a failure towards the address's ban and clearing its count on success.
pub(super) fn check_login(shared: &Shared, user: &str, pass: &str, ip: IpAddr, now: Instant) -> bool {
    let (expected_user, hash) = {
        let st = shared.lock();
        (st.username.clone(), st.password.clone())
    };
    // The hash is checked even for a wrong user name, so the time taken does not tell.
    let pass_ok = hash.verify(pass);
    let ok = constant_time_eq(user.as_bytes(), expected_user.as_bytes()) & pass_ok;
    let mut st = shared.lock();
    if ok {
        st.bans.succeeded(ip);
    } else {
        let (max, ban) = (st.prefs.max_auth_fail_count, Duration::from_secs(st.prefs.ban_duration));
        st.bans.failed(ip, max, ban, now);
    }
    ok
}

fn login(shared: &Shared, p: &Params, ip: IpAddr, now: Instant) -> Response {
    let user = p.get("username").unwrap_or("");
    let pass = p.get("password").unwrap_or("");
    if !check_login(shared, user, pass, ip, now) {
        return Response::text(200, "Fails.");
    }
    let mut st = shared.lock();
    if st.sessions.len() >= MAX_SESSIONS {
        if let Some(oldest) = st.sessions.iter().min_by_key(|(_, s)| s.last_used).map(|(k, _)| k.clone()) {
            st.sessions.remove(&oldest);
        }
    }
    let sid = random_hex(16);
    st.sessions.insert(sid.clone(), ClientSession { last_used: now, maindata: None, peers: None });
    Response::text(200, "Ok.").with_header("Set-Cookie", format!("SID={sid}; {}", cookie_attributes(shared)))
}

/// The session cookie's attributes; over HTTPS it is never sent back over plain HTTP.
fn cookie_attributes(shared: &Shared) -> &'static str {
    if shared.tls.is_some() {
        "HttpOnly; Secure; SameSite=Strict; path=/"
    } else {
        "HttpOnly; SameSite=Strict; path=/"
    }
}

fn valid_category(name: &str) -> bool {
    commands::valid_label(name)
}

fn categories_json(cats: &BTreeMap<String, String>) -> Json {
    let mut out = Json::obj();
    for (name, path) in cats {
        out.set(name, Json::obj().with("name", name.as_str()).with("savePath", path.as_str()));
    }
    out
}

/// `setLocation`: 400 for an empty location, the directory is created first as the documentation
/// describes (403 when that is refused, 409 when it fails otherwise), then the move is the
/// native Set Download Location.
fn set_location(shared: &Shared, p: &Params) -> Response {
    let dir = p.get("location").unwrap_or("").trim();
    if dir.is_empty() {
        return Response::text(400, "Save path is empty");
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        return if e.kind() == std::io::ErrorKind::PermissionDenied {
            Response::text(403, "Cannot write to directory")
        } else {
            Response::text(409, "Cannot create directory")
        };
    }
    let ids = selected(&shared.lock(), p);
    match commands::location(shared, &ids, dir) {
        Ok(()) => Response::ok(),
        Err(e) => Response::text(e.status, e.msg),
    }
}

/// Torrents named by `hashes` (`|`-separated, or `all`), in no particular order.
fn selected(st: &State, p: &Params) -> Vec<TorrentId> {
    let hashes = p.get("hashes").unwrap_or("");
    if hashes == "all" {
        return st.torrents.keys().copied().collect();
    }
    hashes.split('|').filter_map(|h| st.by_hash.get(&h.trim().to_ascii_lowercase()).copied()).collect()
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
        .with("locale", lang::current())
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
        .with("pex", p.pex)
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

/// Applies the `setPreferences` keys we support (unknown keys are ignored) by mapping each onto
/// the native Preferences ids (`prefs.rs`) and committing them together; a key that does not
/// parse is skipped, and when the set fails the Win32 dialog's cross-field checks (a proxy type
/// without a host), each change is tried on its own. Stored like every other preference.
pub(crate) fn apply_preferences(shared: &Shared, st: &mut State, v: &Json) {
    let kib = |j: &Json| {
        j.as_i64().map(|k| Json::from(if k <= 0 { 0 } else { k.min(u32::MAX as i64 / 1024) * 1024 }))
    };
    let clamp = |j: &Json, lo: i64, hi: i64| j.as_i64().map(|n| Json::from(n.clamp(lo, hi)));
    let mut changes = Vec::new();
    for (key, val) in v.entries() {
        let one = |id: &'static str, j: Option<Json>| j.map(|j| vec![(id, j)]).unwrap_or_default();
        let mapped: Vec<(&str, Json)> = match key.as_str() {
            "save_path" => one("dir.download", Some(val.clone())),
            "start_paused_enabled" => one("ui.add_stopped", Some(val.clone())),
            "dl_limit" => one("download_rate_limit", kib(val)),
            "up_limit" => one("upload_rate_limit", kib(val)),
            "alt_dl_limit" => one("alt_dl_limit", kib(val)),
            "alt_up_limit" => one("alt_up_limit", kib(val)),
            "listen_port" => one("listen_port", Some(val.clone())),
            "upnp" => vec![("enable_upnp", val.clone()), ("enable_natpmp", val.clone())],
            "max_connec" => one("connections_limit", clamp(val, -1, i32::MAX as i64)),
            "max_uploads" => one("unchoke_slots_limit", clamp(val, -1, i32::MAX as i64)),
            "dht" => one("enable_dht", Some(val.clone())),
            "lsd" => one("enable_lsd", Some(val.clone())),
            "encryption" => {
                // libtorrent's policy numbers: 0 forced, 1 enabled, 2 disabled
                let policy = match val.as_i64() {
                    Some(0) => Some(1),
                    Some(1) => Some(0),
                    Some(2) => Some(2),
                    _ => None,
                };
                policy
                    .map(|p| vec![("out_enc_policy", Json::from(p)), ("in_enc_policy", Json::from(p))])
                    .unwrap_or_default()
            }
            "max_active_downloads" => one("active_downloads", clamp(val, -1, i32::MAX as i64)),
            "max_active_torrents" => one("active_limit", clamp(val, -1, i32::MAX as i64)),
            "max_active_uploads" => one("active_seeds", clamp(val, -1, i32::MAX as i64)),
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
                one("proxy_type", t.map(|t| Json::from(patorrent_frontend::settings::proxy_to_int(t))))
            }
            "proxy_ip" => one("proxy_hostname", Some(val.clone())),
            "proxy_port" => one("proxy_port", Some(val.clone())),
            "proxy_username" => one("proxy_username", Some(val.clone())),
            "proxy_password" => one("proxy_password", Some(val.clone())),
            "proxy_peer_connections" => one("proxy_peer_connections", Some(val.clone())),
            "web_ui_username" => one("webui.username", Some(val.clone())),
            "web_ui_password" => one("webui.password", Some(val.clone())),
            "bypass_local_auth" => one("webui.bypass_local_auth", Some(val.clone())),
            "web_ui_csrf_protection_enabled" => one("webui.csrf", Some(val.clone())),
            "web_ui_host_header_validation_enabled" => one("webui.host_header_validation", Some(val.clone())),
            "web_ui_domain_list" => one("webui.domain_list", Some(val.clone())),
            "web_ui_max_auth_fail_count" => one("webui.max_auth_fail_count", clamp(val, 0, 1000)),
            "web_ui_ban_duration" => one("webui.ban_duration", clamp(val, 1, 86_400 * 365)),
            "web_ui_session_timeout" => one("webui.session_timeout", clamp(val, 1, 86_400 * 365)),
            _ => Vec::new(),
        };
        for (id, j) in mapped {
            if let Ok(c) = prefs::parse_one(id, &j) {
                changes.push(c);
            }
        }
    }
    if changes.is_empty() {
        return;
    }
    if prefs::commit(shared, st, &changes).is_err() {
        for c in changes {
            let _ = prefs::commit(shared, st, &[c]);
        }
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

/// What one client was last sent by `sync/torrentPeers`: the torrent and the peers by
/// [`peer_key`], each as its full field list, so the next call sends only what changed.
pub struct PeersSnapshot {
    rid: i64,
    hash: String,
    peers: HashMap<String, Json>,
}

/// One peer in `sync/torrentPeers`' documented shape (a subset of the fields: no country or
/// relevance; `flags` are the µTorrent letters of the Peers tab, `connection` is `BT`, `uTP` or
/// `Web`). A web seed's `ip` is its URL, as qBittorrent's is.
fn peer_fields(p: &PeerInfo) -> Json {
    let connection = if p.is_web_seed {
        "Web"
    } else if p.flags.contains(PeerFlags::UTP_SOCKET) {
        "uTP"
    } else {
        "BT"
    };
    Json::obj()
        .with("ip", p.url.clone().unwrap_or_else(|| p.ip.ip().to_string()))
        .with("port", p.ip.port() as i64)
        .with("client", p.client.as_str())
        .with("connection", connection)
        .with("flags", peer_flags(p))
        .with("flags_desc", "")
        .with("country", "")
        .with("country_code", "")
        .with("progress", p.progress_ppm.min(1_000_000) as f64 / 1_000_000.0)
        .with("dl_speed", p.down_speed)
        .with("up_speed", p.up_speed)
        .with("downloaded", p.total_download)
        .with("uploaded", p.total_upload)
        .with("relevance", 0.0f64)
        .with("files", "")
}

/// A peer's key in `sync/torrentPeers`: `ip:port`, or a web seed's URL (several URLs can share
/// a server).
fn peer_key(p: &PeerInfo) -> String {
    p.url.clone().unwrap_or_else(|| p.ip.to_string())
}

/// `sync/torrentPeers`: the torrent's connected peers, a full list for `rid=0` (or a rid the
/// server does not know, or another torrent than last time), else the changed fields per peer
/// and `peers_removed`; `show_flags` is always true.
fn torrent_peers(shared: &Shared, key: &str, p: &Params) -> Response {
    let Some(id) = one(&shared.lock(), p) else { return not_found() };
    let Some(Reply::Peers(list)) = shared.query(id, ReplyKind::Peers, |h| h.torrent(id).post_peer_info())
    else {
        return not_found();
    };
    let rid = p.int("rid").unwrap_or(0);
    let mut guard = shared.lock();
    let st = &mut *guard;
    let hash = st.torrents[&id].hash.clone();
    let old = st.sessions.get_mut(key).and_then(|s| s.peers.take());
    let full = old.as_ref().is_none_or(|o| rid == 0 || o.rid != rid || o.hash != hash);
    let new_rid = old.as_ref().map_or(0, |o| o.rid) + 1;
    let mut old = if full { None } else { old };
    let mut out = Json::obj().with("rid", new_rid);
    if full {
        out.set("full_update", true);
    }
    out.set("show_flags", true);
    let mut peers = Json::obj();
    let mut snap = HashMap::with_capacity(list.len());
    for peer in &list {
        let addr = peer_key(peer);
        let now = peer_fields(peer);
        match old.as_mut().and_then(|o| o.peers.remove(&addr)) {
            Some(prev) => {
                let mut diff = Json::obj();
                for (name, v) in now.entries() {
                    if prev.get(name) != Some(v) {
                        diff.set(name, v.clone());
                    }
                }
                if !diff.entries().is_empty() {
                    peers.set(&addr, diff);
                }
            }
            None => peers.set(&addr, now.clone()),
        }
        snap.insert(addr, now);
    }
    if full || !peers.entries().is_empty() {
        out.set("peers", peers);
    }
    if let Some(o) = &old {
        let removed: Vec<Json> = o.peers.keys().map(|a| Json::Str(a.clone())).collect();
        if !removed.is_empty() {
            out.set("peers_removed", Json::Arr(removed));
        }
    }
    if let Some(s) = st.sessions.get_mut(key) {
        s.peers = Some(PeersSnapshot { rid: new_rid, hash, peers: snap });
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
    let Some(id) = one(&shared.lock(), p) else { return not_found() };
    // asked of the engine the first time (a torrent restored from the state directory)
    let meta = commands::meta(shared, id);
    let st = shared.lock();
    let Some(e) = st.torrents.get(&id) else { return not_found() };
    let s = &e.status;
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
    let Some(id) = one(&shared.lock(), p) else { return not_found() };
    let private = commands::meta(shared, id).private;
    let (dht, lsd, pex) = {
        let st = shared.lock();
        let Some(e) = st.torrents.get(&id) else { return not_found() };
        let f = e.status.flags;
        (
            st.settings.enable_dht && !private && !f.contains(TorrentFlags::DISABLE_DHT),
            st.settings.enable_lsd && !private && !f.contains(TorrentFlags::DISABLE_LSD),
            !private && !f.contains(TorrentFlags::DISABLE_PEX),
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
        Some(Reply::Pieces(_, a, _)) => a,
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
    let Some(Reply::Pieces(have, _, downloading)) =
        shared.query(id, ReplyKind::Pieces, |h| h.torrent(id).post_piece_info())
    else {
        return not_found();
    };
    // 0 missing, 1 being downloaded, 2 had
    let mut v: Vec<Json> = (0..have.len()).map(|i| Json::Int(if have.get(i) { 2 } else { 0 })).collect();
    for p in downloading {
        if let Some(s) = v.get_mut(p.piece as usize) {
            *s = Json::Int(1);
        }
    }
    Response::json(&Json::Arr(v))
}

/// `torrents/addPeers`: `peers` (`|`-separated `ip:port`, IPv6 as `[ip]:port`) connected to
/// on every torrent `hashes` names (libtorrent `connect_peer`); 400 when none of the peers is
/// an address.
fn add_peers(shared: &Shared, p: &Params) -> Response {
    let peers: Vec<std::net::SocketAddr> = p
        .get("peers")
        .unwrap_or("")
        .split('|')
        .filter_map(|s| s.trim().parse().ok())
        .filter(|a: &std::net::SocketAddr| a.port() != 0)
        .collect();
    if peers.is_empty() {
        return Response::text(400, "No valid peers were specified");
    }
    let ids = selected(&shared.lock(), p);
    for id in ids {
        let t = shared.handle.torrent(id);
        for &a in &peers {
            t.connect_peer(a);
        }
    }
    Response::ok()
}

/// `torrents/filePrio`: the API's priorities (0, 1, 6, 7) onto the shared file command.
fn file_prio(shared: &Shared, p: &Params) -> Response {
    let prio = match p.int("priority") {
        Some(0) => DownloadPriority::DONT_DOWNLOAD,
        Some(1) => DownloadPriority::NORMAL,
        Some(6) => DownloadPriority(6),
        Some(7) => DownloadPriority::TOP,
        _ => return Response::text(400, "invalid priority"),
    };
    let mut prios = Vec::new();
    for s in p.get("id").unwrap_or("").split('|') {
        match s.trim().parse::<u32>() {
            Ok(i) => prios.push((i, prio)),
            Err(_) => return Response::text(400, "file id is not an integer"),
        }
    }
    let Some(id) = one(&shared.lock(), p) else { return not_found() };
    match commands::files(shared, id, &prios) {
        Ok(()) => Response::ok(),
        Err(e) => Response::text(e.status, e.msg),
    }
}

/// `torrents/renameFile` (`oldPath` names one file) and `torrents/renameFolder` (every file
/// under `oldPath`); paths as `torrents/files` names them, torrent folder included. The rename
/// runs in the background, as in qBittorrent.
fn rename(shared: &Shared, p: &Params, folder: bool) -> Response {
    let (Some(old), Some(new)) = (p.get("oldPath"), p.get("newPath")) else {
        return Response::text(400, "missing oldPath or newPath");
    };
    let Some(id) = one(&shared.lock(), p) else { return not_found() };
    match commands::rename(shared, id, old, new, Some(folder)) {
        Ok(()) => Response::ok(),
        Err(e) => Response::text(e.status, e.msg.replace("new path", "newPath")),
    }
}

/// `torrents/add`: every URL (magnet, bare hash, `http://`) and uploaded file, with the
/// documented parameters mapped onto the Add dialog's options; one failure among several
/// sources does not stop the rest, as in qBittorrent.
fn add(shared: &Shared, p: &Params) -> Response {
    let mut sources: Vec<Source> = Vec::new();
    let mut invalid_file = false;
    for url in p.get("urls").unwrap_or("").split('\n').map(str::trim).filter(|u| !u.is_empty()) {
        if let Ok(s) = Source::from_url(shared, url) {
            sources.push(s);
        }
    }
    for f in &p.files {
        match Source::from_torrent_bytes(f.data.clone()) {
            Ok(s) => sources.push(s),
            Err(_) => invalid_file = true,
        }
    }
    if sources.is_empty() {
        return if invalid_file {
            Response::text(415, "Torrent file is not valid.")
        } else {
            Response::text(200, "Fails.")
        };
    }
    let category = p.get("category").map(str::trim).unwrap_or("");
    let start = p.get("stopped").or(p.get("paused")).map(|v| !v.eq_ignore_ascii_case("true"));
    let lim = |name: &str| p.int(name).map_or(0, |v| v.clamp(0, u32::MAX as i64) as u32);
    let o = AddOptions {
        save: p.get("savepath").map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned),
        name: p.get("rename").map(str::trim).filter(|r| !r.is_empty()).map(str::to_owned),
        label: if valid_category(category) { category.to_owned() } else { String::new() },
        start,
        skip_check: p.flag("skip_checking"),
        top: false,
        priorities: Default::default(),
        sequential: p.flag("sequentialDownload"),
        download_limit: lim("dlLimit"),
        upload_limit: lim("upLimit"),
        dialog: false,
    };
    for source in sources {
        if let Err(e) = commands::add(shared, source, &o) {
            return Response::text(e.status, e.msg);
        }
    }
    Response::text(200, "Ok.")
}

#[cfg(test)]
mod tests {
    use super::*;

    use patorrent_frontend::lang::S;

    /// A web seed's row names it by its URL, as qBittorrent's does; a peer by its address.
    #[test]
    fn web_seed_peers_are_their_urls() {
        let peer = PeerInfo {
            ip: "10.0.0.1:443".parse().unwrap(),
            client: "nginx".into(),
            flags: PeerFlags::empty(),
            source: patorrent_core::PeerSource::empty(),
            progress_ppm: 1_000_000,
            down_speed: 0,
            up_speed: 0,
            total_download: 0,
            total_upload: 0,
            download_queue_length: 0,
            upload_queue_length: 0,
            num_hashfails: 0,
            is_web_seed: false,
            url: None,
        };
        let f = peer_fields(&peer);
        assert_eq!(f.get("ip").and_then(Json::as_str), Some("10.0.0.1"));
        assert_eq!(f.get("connection").and_then(Json::as_str), Some("BT"));
        assert_eq!(peer_key(&peer), "10.0.0.1:443");
        let url = "https://mirror.example/ubuntu.iso";
        let seed = PeerInfo { is_web_seed: true, url: Some(url.into()), ..peer };
        let f = peer_fields(&seed);
        assert_eq!(f.get("ip").and_then(Json::as_str), Some(url));
        assert_eq!(f.get("connection").and_then(Json::as_str), Some("Web"));
        assert_eq!(f.get("client").and_then(Json::as_str), Some("nginx"));
        assert_eq!(f.get("port").and_then(Json::as_f64), Some(443.0));
        assert_eq!(peer_key(&seed), url);
    }

    #[test]
    fn the_page_has_the_application_icon() {
        let f = static_file("/favicon.png").expect("served");
        assert_eq!((f.content_type, &f.body[..8]), ("image/png", &b"\x89PNG\r\n\x1a\n"[..]));
        assert!(
            include_str!("page.html").contains(r#"<link rel="icon" type="image/png" href="favicon.png">"#)
        );
    }

    /// Every key the page names (`data-t`, `data-t-title`, `data-t-aria`, `data-t-ph`) is in
    /// the string table, and the English the page carries for a browser without scripts is the
    /// table's, without its mnemonics.
    #[test]
    fn the_page_names_known_strings() {
        let page = include_str!("page.html");
        let mut found = 0;
        for attr in ["data-t=\"", "data-t-title=\"", "data-t-aria=\"", "data-t-ph=\""] {
            let mut rest = page;
            while let Some(i) = rest.find(attr) {
                rest = &rest[i + attr.len()..];
                let key = &rest[..rest.find('"').unwrap()];
                let s = S::from_key(key).unwrap_or_else(|| panic!("page.html: unknown key {key}"));
                if attr == "data-t=\"" {
                    // the element's text, after the end of its start tag
                    let text = &rest[rest.find('>').unwrap() + 1..];
                    let text = &text[..text.find('<').unwrap()];
                    assert_eq!(text, lang::plain(s.english()), "page.html: {key}");
                }
                found += 1;
            }
        }
        assert!(found > 100, "{found}");
    }

    /// The keys app.js asks for: `T("Key")`, and the keys it keeps in lists for `T` (any
    /// CamelCase string literal of two words or more must be a key, but for a few DOM names).
    #[test]
    fn the_script_names_known_strings() {
        let js = include_str!("app.js");
        const NOT_KEYS: [&str; 8] = [
            "PaTorrent",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "PageUp",
            "PageDown",
            "ContextMenu",
        ];
        let mut found = 0;
        for (i, _) in js.match_indices('"') {
            let rest = &js[i + 1..];
            let Some(end) = rest.find('"') else { break };
            let lit = &rest[..end];
            let camel = lit.len() > 3
                && lit.chars().all(|c| c.is_ascii_alphanumeric())
                && lit.starts_with(|c: char| c.is_ascii_uppercase())
                && lit.chars().skip(1).any(|c| c.is_ascii_uppercase())
                && lit.chars().any(|c| c.is_ascii_lowercase());
            if camel && !NOT_KEYS.contains(&lit) {
                assert!(S::from_key(lit).is_some(), "app.js: unknown key {lit}");
                found += 1;
            }
        }
        assert!(found > 100, "{found}");
        // the dynamic ones are list entries; a T( call with a literal names its key directly
        for (i, _) in js.match_indices("T(\"") {
            let key = &js[i + 3..][..js[i + 3..].find('"').unwrap()];
            assert!(S::from_key(key).is_some(), "app.js: unknown key {key}");
        }
    }

    #[test]
    fn lang_js_carries_the_table_without_mnemonics() {
        let js = lang_js();
        let body = js.strip_prefix("window.PA_LANG = ").and_then(|b| b.strip_suffix(";\n")).unwrap();
        let table = json::parse(body).unwrap();
        assert_eq!(table.get("code").and_then(Json::as_str), Some("en"));
        assert_eq!(table.get("dir").and_then(Json::as_str), Some("ltr"));
        let texts = table.get("t").unwrap();
        assert_eq!(texts.entries().len(), S::ALL.len());
        assert_eq!(texts.get("MenuFile").and_then(Json::as_str), Some("File"));
        assert_eq!(texts.get("GenConnected").and_then(Json::as_str), Some("{count} of {total} connected"));
        assert!(static_file("/lang.js").is_some_and(|f| f.body == js.as_bytes()));
    }

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
        assert!(valid_category("tv/hd") && !valid_category("/tv") && !valid_category("a//b"));
    }
}
