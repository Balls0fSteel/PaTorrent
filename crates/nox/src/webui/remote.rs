//! Remote sessions (`patorrent_core::remote`): a GUI driving this nox's engine over the Web
//! UI's port (`GET /pa/remote`, an HTTP upgrade). The client's commands go to the engine as
//! they are, except the two status requests, which are answered from the cache with what
//! changed since this client's last one, and settings, which take the Preferences dialog's
//! path (validated, applied, stored). Every alert the engine sends (but its status replies,
//! which belong to the cache) is queued for every remote client by [`Shared::fan_out`].

use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use patorrent_core::Alert;
use patorrent_core::remote::{self, FrameReader, Hello, Request};

use super::api::{check_login, host_allowed};
use super::http::{Head, Response};
use super::server::Conn;
use super::{Shared, prefs};

/// Alerts queued for one client beyond this mean it does not keep up: it is disconnected
/// (and resynchronises when it reconnects) rather than holding memory without bound.
const MAX_QUEUED: usize = 100_000;

/// One connected remote client's queue.
#[derive(Default)]
pub(crate) struct Client {
    queue: Mutex<VecDeque<Alert>>,
    /// Disconnect it: its queue overflowed, or the password changed.
    ended: AtomicBool,
}

impl Shared {
    /// Queues a batch of the engine's alerts for every remote client: all but the replies to
    /// this nox's own status requests.
    pub(crate) fn fan_out(&self, alerts: &[Alert]) {
        let clients = self.remotes.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for c in clients {
            let mut q = c.queue.lock().unwrap_or_else(|e| e.into_inner());
            for a in alerts {
                if !matches!(a, Alert::StateUpdate(_) | Alert::SessionStats(_)) {
                    q.push_back(a.clone());
                }
            }
            if q.len() > MAX_QUEUED {
                q.clear();
                c.ended.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Disconnects every remote client, within a read timeout (the password changed, SEC-24).
    pub(crate) fn end_remotes(&self) {
        for c in self.remotes.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            c.ended.store(true, Ordering::SeqCst);
        }
    }
}

/// The checks a `GET /pa/remote` upgrade must pass: the ban list, the `Host` header, an
/// `Origin` (only a browser sends one: it must be this server's, as for the WebSocket), the
/// upgrade headers and protocol version, then the login: Basic credentials checked like the
/// login form (a failure counts towards the ban), or a valid session cookie, or a loopback
/// client with the local-auth bypass on.
pub(super) fn handshake(shared: &Shared, head: &Head, ip: IpAddr) -> Result<(), Response> {
    let ip = super::api::client_ip(head, ip);
    let now = Instant::now();
    {
        let mut st = shared.lock();
        if st.bans.is_banned(ip, now) {
            return Err(Response::text(
                403,
                "Your IP address has been banned after too many failed authentication attempts.",
            ));
        }
        if st.prefs.host_header_validation && !host_allowed(head.header("host"), &st.prefs.domain_list) {
            return Err(Response::text(400, super::api::HOST_REFUSED));
        }
        if st.prefs.csrf_protection && head.header("origin").is_some() && !super::api::csrf_ok(head) {
            return Err(Response::text(403, "Forbidden: origin not allowed"));
        }
    }
    let upgrade = head.header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case(remote::UPGRADE));
    let connection = head
        .header("connection")
        .is_some_and(|c| c.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade")));
    if head.method != "GET" || !upgrade || !connection {
        return Err(Response::text(400, "Bad Request: not a remote session upgrade"));
    }
    if head.header(&remote::VERSION_HEADER.to_ascii_lowercase())
        != Some(remote::PROTOCOL.to_string().as_str())
    {
        return Err(Response::text(426, format!("this server speaks remote protocol {}", remote::PROTOCOL))
            .with_header(remote::VERSION_HEADER, remote::PROTOCOL.to_string()));
    }
    let basic = head
        .header("authorization")
        .and_then(|a| a.strip_prefix("Basic ").or_else(|| a.strip_prefix("basic ")))
        .and_then(|b| patorrent_core::util::string::base64decode(b.trim().as_bytes()))
        .and_then(|b| String::from_utf8(b).ok());
    let ok = match basic.as_deref().and_then(|b| b.split_once(':')) {
        Some((user, pass)) => check_login(shared, user, pass, ip, now),
        None => super::api::authorized(shared, head, ip),
    };
    if ok { Ok(()) } else { Err(Response::text(401, "Unauthorized")) }
}

/// The `101` answer.
pub(super) fn switching_protocols() -> Vec<u8> {
    format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: {}\r\n\r\n", remote::UPGRADE)
        .into_bytes()
}

/// Removes the client from the fan-out when its connection ends, however it ends.
struct Registered<'a>(&'a Shared, Arc<Client>);

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.0.remotes.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| !Arc::ptr_eq(c, &self.1));
    }
}

/// The connection after the upgrade, on its own thread: the hello, then the client's commands
/// as they come (read with a short timeout) and its queued alerts after each read. `buf` holds
/// what arrived after the upgrade request.
pub(super) fn run(mut stream: Conn, buf: Vec<u8>, shared: &Shared) {
    use std::io::Write;
    let client = Arc::new(Client::default());
    shared.remotes.lock().unwrap_or_else(|e| e.into_inner()).push(client.clone());
    let _registered = Registered(shared, client.clone());
    let mut out = Vec::new();
    {
        let st = shared.lock();
        let hello = Hello {
            server_version: patorrent_core::VERSION.to_owned(),
            save_path: st.prefs.save_path.clone(),
            settings: st.settings.clone(),
        };
        remote::hello_frame(&hello, &mut out);
    }
    if stream.write_all(&out).and_then(|()| stream.flush()).is_err() {
        return;
    }
    stream.set_read_timeout(super::server::WS_POLL);
    // client frames carry .torrent files: the upload limit applies, with room for the rest
    let mut reader = FrameReader::new(buf, shared.limits.max_body.saturating_add(1 << 20));
    // the cache's torrents with a version past this have not been sent to this client
    let mut sent_until = 0u64;
    loop {
        out.clear();
        match reader.read_from(&mut stream) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) if remote::is_timeout(&e) => {}
            Err(_) => return,
        }
        loop {
            let frame = match reader.next_frame() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(_) => return,
            };
            match remote::dispatch(&shared.handle, &frame) {
                None => return,
                Some(Request::Forwarded) => {}
                Some(Request::PostTorrentUpdates) => {
                    let st = shared.lock();
                    let changed: Vec<_> = st
                        .torrents
                        .values()
                        .filter(|e| e.version >= sent_until)
                        .map(|e| e.status.clone())
                        .collect();
                    sent_until = st.next_version;
                    drop(st);
                    remote::state_update_frame(changed, &mut out);
                }
                Some(Request::PostSessionStats) => {
                    let stats = shared.lock().stats;
                    remote::session_stats_frame(stats, &mut out);
                }
                Some(Request::ApplySettings(s)) => {
                    let mut st = shared.lock();
                    if let Err((field, why)) = prefs::commit_settings(shared, &mut st, &s) {
                        let a =
                            Alert::Log { id: None, message: format!("settings not applied: {field}: {why}") };
                        remote::alert_frame(&a, &mut out);
                    }
                }
            }
        }
        let queued = std::mem::take(&mut *client.queue.lock().unwrap_or_else(|e| e.into_inner()));
        for a in &queued {
            remote::alert_frame(a, &mut out);
        }
        if client.ended.load(Ordering::SeqCst) {
            return;
        }
        if !out.is_empty() && stream.write_all(&out).and_then(|()| stream.flush()).is_err() {
            return;
        }
    }
}
