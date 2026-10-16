// Ported from libtorrent-rasterbar 2.0.11: src/upnp.cpp, include/libtorrent/upnp.hpp,
// test/test_upnp.cpp (the loopback IGD test)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the asio timers are deadlines polled by `service`; `http_connection` is a small
// non-blocking client of its own (one request per root device at a time, IP-literal hosts
// only); the callback is a queue of `PortmapEvent`s and log lines the engine drains; devices
// live in a `Vec` (indices are the HTTP reactor tokens) instead of a `std::set`.

//! A UPnP IGD port mapper for one IPv4 interface: finds root devices with SSDP, reads their
//! description for a WANIPConnection / WANPPPConnection control URL, asks for the external
//! address and adds, refreshes and deletes the session's port mappings with SOAP.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use mio::net::{TcpStream, UdpSocket};
use mio::{Interest, Registry, Token};

use super::portmap::{Action, PortmapEvent};
use crate::alert::Protocol;
use crate::net::enum_net::match_addr_mask;
use crate::net::sys;
use crate::proto::http;
use crate::proto::upnp;

/// libtorrent `max_global_mappings`, and the root device cap of `on_reply`.
const MAX_MAPPINGS: usize = 50;
const MAX_DEVICES: usize = 50;
/// SSDP searches: 2 s, 4 s, ... apart, up to twelve (four once a device answered).
const MAX_SEARCHES: u32 = 12;
const SEARCHES_WITH_DEVICE: u32 = 4;
const XML_TIMEOUT: Duration = Duration::from_secs(30);
const SOAP_TIMEOUT: Duration = Duration::from_secs(10);
/// A request in flight makes its mapping "expire" in 30 s, so a lost reply is retried.
const PENDING_EXPIRY: Duration = Duration::from_secs(30);
const MAX_FAILCOUNT: u32 = 5;

#[derive(Clone, Debug)]
pub(super) struct UpnpSettings {
    pub ignore_nonrouters: bool,
    pub lease_duration: u32,
    /// The mapping's description on the router.
    pub user_agent: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    /// The device description (`GET` on the root device URL).
    Xml,
    GetIp,
    Map(usize),
    Unmap(usize),
}

enum HttpError {
    Failed(String),
    /// The header never finished.
    Incomplete,
}

struct HttpConn {
    stream: TcpStream,
    kind: Request,
    /// Built once the connection is up (a mapping names the local address it was made from).
    request: Vec<u8>,
    written: usize,
    buf: Vec<u8>,
    connected: bool,
    deadline: Instant,
}

struct Mapping {
    protocol: Option<Protocol>,
    external_port: u16,
    local_port: u16,
    act: Action,
    /// `None`: never (libtorrent `max_time()`).
    expires: Option<Instant>,
    failcount: u32,
}

struct RootDevice {
    /// The interface url, through which the list of supported interfaces are fetched.
    url: String,
    /// The url to the WANIP or WANPPP interface.
    control_url: String,
    /// Either the WANIP namespace or the WANPPP namespace.
    service_namespace: String,
    mapping: Vec<Mapping>,
    /// The hostname, port and path of `url`, or of `control_url` once it has been found.
    hostname: String,
    port: u16,
    path: String,
    external_ip: Option<IpAddr>,
    /// False if the router doesn't support lease durations.
    use_lease_duration: bool,
    disabled: bool,
    conn: Option<HttpConn>,
}

struct GlobalMapping {
    protocol: Option<Protocol>,
    external_port: u16,
    local_port: u16,
}

pub(super) struct Upnp {
    settings: UpnpSettings,
    mappings: Vec<GlobalMapping>,
    devices: Vec<RootDevice>,
    retry_count: u32,
    multicast: Option<UdpSocket>,
    unicast: Option<UdpSocket>,
    /// libtorrent `m_broadcast_timer`, `m_refresh_timer` and `m_map_timer`.
    broadcast_at: Option<Instant>,
    refresh_at: Option<Instant>,
    map_at: Option<Instant>,
    disabled: bool,
    closing: bool,
    model: String,
    /// The network this mapper belongs to; it talks to no other.
    listen_address: Ipv4Addr,
    netmask: Ipv4Addr,
    /// Device `i`'s HTTP connection is registered as `Token(http_token_base + i)`.
    http_token_base: usize,
    buf: Vec<u8>,
    pub events: Vec<PortmapEvent>,
    pub log: Vec<String>,
}

impl Upnp {
    pub fn new(
        settings: UpnpSettings,
        listen_address: Ipv4Addr,
        netmask: Ipv4Addr,
        http_token_base: usize,
    ) -> Upnp {
        Upnp {
            settings,
            mappings: Vec::new(),
            devices: Vec::new(),
            retry_count: 0,
            multicast: None,
            unicast: None,
            broadcast_at: None,
            refresh_at: None,
            map_at: None,
            disabled: false,
            closing: false,
            model: String::new(),
            listen_address,
            netmask,
            http_token_base,
            buf: vec![0; 1500],
            events: Vec::new(),
            log: Vec::new(),
        }
    }

    pub fn set_settings(&mut self, settings: UpnpSettings) {
        self.settings = settings;
    }

    /// The model name the router advertised, once its description was read.
    #[cfg(test)]
    fn router_model(&self) -> &str {
        &self.model
    }

    pub fn http_token_base(&self) -> usize {
        self.http_token_base
    }

    pub fn owns_http_token(&self, token: usize) -> bool {
        (self.http_token_base..self.http_token_base + MAX_DEVICES).contains(&token)
    }

    /// Whether a delete request is still to be sent or answered: the engine waits for these
    /// at shutdown.
    pub fn pending(&self) -> bool {
        self.devices.iter().any(|d| d.conn.is_some() || d.mapping.iter().any(|m| m.act == Action::Del))
    }

    pub fn deadline(&self) -> Option<Instant> {
        let mut d = self.broadcast_at;
        for t in [self.refresh_at, self.map_at] {
            d = match (d, t) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        for dev in &self.devices {
            if let Some(c) = &dev.conn {
                d = Some(d.map_or(c.deadline, |x| x.min(c.deadline)));
            }
        }
        d
    }

    /// libtorrent `upnp::start`: the SSDP sockets, then the first search.
    pub fn start(&mut self, registry: &Registry, multicast_token: Token, unicast_token: Token, now: Instant) {
        match self.open_multicast_socket(registry, multicast_token) {
            Ok(s) => self.multicast = Some(s),
            Err(e) => {
                self.log.push(format!("failed to open multicast socket: \"{e}\""));
                self.disabled = true;
                return;
            }
        }
        match self.open_unicast_socket(registry, unicast_token) {
            Ok(s) => self.unicast = Some(s),
            Err(e) => {
                self.log.push(format!("failed to open unicast socket: \"{e}\""));
                self.disabled = true;
                return;
            }
        }
        self.discover_device_impl(now);
    }

    fn open_multicast_socket(&self, registry: &Registry, token: Token) -> io::Result<UdpSocket> {
        let s = sys::udp_socket(SocketAddr::new(self.listen_address.into(), upnp::SSDP_PORT), true)?;
        s.join_multicast_v4(&upnp::SSDP_MULTICAST, &self.listen_address)?;
        s.set_multicast_ttl_v4(255)?;
        s.set_multicast_loop_v4(true)?;
        sys::set_multicast_if_v4(&s, self.listen_address)?;
        s.set_nonblocking(true)?;
        let mut s = UdpSocket::from_std(s);
        registry.register(&mut s, token, Interest::READABLE)?;
        Ok(s)
    }

    fn open_unicast_socket(&self, registry: &Registry, token: Token) -> io::Result<UdpSocket> {
        let mut s = UdpSocket::bind(SocketAddr::new(self.listen_address.into(), 0))?;
        registry.register(&mut s, token, Interest::READABLE)?;
        Ok(s)
    }

    fn discover_device_impl(&mut self, now: Instant) {
        let group = SocketAddr::new(upnp::SSDP_MULTICAST.into(), upnp::SSDP_PORT);
        let send = |s: &Option<UdpSocket>| -> io::Result<()> {
            match s {
                Some(s) => s.send_to(upnp::MSEARCH, group).map(|_| ()),
                None => Err(io::Error::new(io::ErrorKind::NotConnected, "no socket")),
            }
        };
        let (mcast, ucast) = (send(&self.multicast), send(&self.unicast));
        if let (Err(m), Err(u)) = (&mcast, &ucast) {
            self.log.push(format!("multicast send failed: \"{m}\" and \"{u}\". Aborting."));
            self.disable(m.to_string());
            return;
        }
        self.retry_count += 1;
        self.broadcast_at = Some(now + Duration::from_secs(2 * self.retry_count as u64));
        self.log.push("broadcasting search for rootdevice".into());
    }

    /// Returns the mapping's index, or `None` when the mapper is disabled or full.
    pub fn add_mapping(
        &mut self,
        p: Protocol,
        external_port: u16,
        local_port: u16,
        registry: &Registry,
        now: Instant,
    ) -> Option<usize> {
        // external port 0 means _every_ port
        debug_assert!(external_port != 0);
        self.log.push(format!(
            "adding port map: [ protocol: {} ext_port: {external_port} local_port: {local_port} ] {}",
            upnp::protocol_name(p),
            if self.disabled { "DISABLED" } else { "" }
        ));
        if self.disabled {
            return None;
        }
        let index = match self.mappings.iter().position(|m| m.protocol.is_none()) {
            Some(i) => i,
            None => {
                if self.mappings.len() >= MAX_MAPPINGS {
                    self.log.push("too many mappings registered".into());
                    return None;
                }
                self.mappings.push(GlobalMapping { protocol: None, external_port: 0, local_port: 0 });
                self.mappings.len() - 1
            }
        };
        self.mappings[index] = GlobalMapping { protocol: Some(p), external_port, local_port };
        for di in 0..self.devices.len() {
            let d = &mut self.devices[di];
            if d.disabled {
                continue;
            }
            while d.mapping.len() <= index {
                d.mapping.push(Mapping {
                    protocol: None,
                    external_port: 0,
                    local_port: 0,
                    act: Action::None,
                    expires: None,
                    failcount: 0,
                });
            }
            let m = &mut d.mapping[index];
            m.act = Action::Add;
            m.protocol = Some(p);
            m.external_port = external_port;
            m.local_port = local_port;
            if !d.service_namespace.is_empty() {
                self.update_map(di, index, registry, now);
            }
        }
        Some(index)
    }

    pub fn delete_mapping(&mut self, index: usize, registry: &Registry, now: Instant) {
        let Some(m) = self.mappings.get(index) else { return };
        let Some(p) = m.protocol else { return };
        self.log.push(format!(
            "deleting port map: [ protocol: {} ext_port: {} local_port: {} ]",
            upnp::protocol_name(p),
            m.external_port,
            m.local_port
        ));
        for di in 0..self.devices.len() {
            let d = &mut self.devices[di];
            if d.disabled || index >= d.mapping.len() {
                continue;
            }
            d.mapping[index].act = Action::Del;
            if !d.service_namespace.is_empty() {
                self.update_map(di, index, registry, now);
            }
        }
    }

    /// Runs the timers: searches, the map check, lease refreshes and HTTP timeouts.
    pub fn service(&mut self, registry: &Registry, now: Instant) {
        if self.broadcast_at.is_some_and(|t| now >= t) {
            self.broadcast_at = None;
            self.resend_request(registry, now);
        }
        if self.map_at.is_some_and(|t| now >= t) {
            self.map_at = None;
            if !self.closing {
                self.try_map_upnp(registry, now);
            }
        }
        if self.refresh_at.is_some_and(|t| now >= t) {
            self.refresh_at = None;
            self.on_expire(registry, now);
        }
        for di in 0..self.devices.len() {
            if self.devices[di].conn.as_ref().is_some_and(|c| now >= c.deadline) {
                self.finish_request(di, Err(HttpError::Failed("timed out".into())), registry, now);
            }
        }
    }

    /// libtorrent `resend_request`: another search, or the description of devices found.
    fn resend_request(&mut self, registry: &Registry, now: Instant) {
        if self.closing {
            return;
        }
        if self.retry_count < MAX_SEARCHES
            && (self.devices.is_empty() || self.retry_count < SEARCHES_WITH_DEVICE)
        {
            self.discover_device_impl(now);
            return;
        }
        if self.devices.is_empty() {
            self.disable("no router found".into());
            return;
        }
        for di in 0..self.devices.len() {
            let d = &self.devices[di];
            if !d.control_url.is_empty() || d.conn.is_some() || d.disabled {
                continue;
            }
            // we don't have a WANIP or WANPPP url for this device, ask for it
            self.connect(di, registry, now);
        }
    }

    fn connect(&mut self, di: usize, registry: &Registry, now: Instant) {
        self.log.push(format!("connecting to: {}", self.devices[di].url));
        self.start_request(di, Request::Xml, XML_TIMEOUT, registry, now);
    }

    /// Reads SSDP replies from one of the sockets.
    pub fn on_readable(&mut self, multicast: bool, registry: &Registry, now: Instant) {
        loop {
            let sock = if multicast { self.multicast.as_ref() } else { self.unicast.as_ref() };
            let Some(s) = sock else { return };
            let mut buf = std::mem::take(&mut self.buf);
            let r = s.recv_from(&mut buf);
            self.buf = buf;
            match r {
                Ok((n, from)) => {
                    let packet = self.buf[..n].to_vec();
                    self.on_reply(&packet, from, registry, now);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::ConnectionReset) => {
                }
                Err(_) => return,
            }
        }
    }

    /// libtorrent `upnp::on_reply`: a search response or a NOTIFY names a root device.
    fn on_reply(&mut self, buf: &[u8], from: SocketAddr, registry: &Registry, now: Instant) {
        if self.closing {
            return;
        }
        if self.settings.ignore_nonrouters
            && !match_addr_mask(self.listen_address.into(), from.ip(), self.netmask.into())
        {
            self.log.push(format!(
                "ignoring response from: {from}. IP is not on local network. (addr: {} mask: {})",
                self.listen_address, self.netmask
            ));
            return;
        }
        let (_, url) = match upnp::parse_ssdp(buf) {
            Ok(r) => r,
            Err(upnp::Invalid::Malformed) => {
                self.log.push(format!("received malformed HTTP from: {from}"));
                return;
            }
            Err(upnp::Invalid::Status) => {
                let first = String::from_utf8_lossy(buf);
                let first = first.lines().next().unwrap_or_default();
                if !first.starts_with("HTTP/") {
                    // our own M-SEARCH comes back through multicast loopback: say nothing
                    if !first.starts_with("M-SEARCH") {
                        self.log.push(format!("HTTP method {first} from {from}"));
                    }
                } else {
                    self.log.push(format!("HTTP status {first} from {from}"));
                }
                return;
            }
            Err(upnp::Invalid::Incomplete) => {
                self.log.push(format!("incomplete HTTP packet from {from}"));
                return;
            }
            Err(upnp::Invalid::MissingLocation) => {
                self.log.push(format!("missing location header from {from}"));
                return;
            }
        };

        if !self.devices.iter().any(|d| d.url == url) {
            // we don't have this device in our list. Add it
            let parts = match crate::util::string::parse_url_components(&url) {
                Ok(p) => p,
                Err(e) => {
                    self.log.push(format!("invalid URL {url} from {from}: {e:?}"));
                    return;
                }
            };
            if !parts.protocol.eq_ignore_ascii_case("http") {
                self.log.push(format!("unsupported protocol {} from {from}", parts.protocol));
                return;
            }
            let port = match parts.port {
                Some(0) => {
                    self.log.push(format!("URL with port 0 from {from}"));
                    return;
                }
                Some(p) => p,
                None => 80,
            };
            self.log.push(format!("found rootdevice: {url} ({})", self.devices.len()));
            if self.devices.len() >= MAX_DEVICES {
                self.log.push(format!("too many rootdevices: ({}). Ignoring {url}", self.devices.len()));
                return;
            }
            let mapping = self
                .mappings
                .iter()
                .map(|g| Mapping {
                    protocol: g.protocol,
                    external_port: g.external_port,
                    local_port: g.local_port,
                    act: Action::Add,
                    expires: None,
                    failcount: 0,
                })
                .collect();
            self.devices.push(RootDevice {
                url,
                control_url: String::new(),
                service_namespace: String::new(),
                mapping,
                hostname: parts.host,
                port,
                path: if parts.path.is_empty() { "/".into() } else { parts.path },
                external_ip: None,
                use_lease_duration: true,
                disabled: false,
                conn: None,
            });
        }

        // iterate over the devices we know and connect and issue the mappings
        self.try_map_upnp(registry, now);

        // check back in a little bit to see if we have seen any devices at one of our default
        // routes. If not, we want to override ignoring them and use them instead (better than
        // not working).
        self.map_at = Some(now + Duration::from_secs(1));
    }

    fn try_map_upnp(&mut self, registry: &Registry, now: Instant) {
        for di in 0..self.devices.len() {
            let d = &self.devices[di];
            if d.control_url.is_empty() && d.conn.is_none() && !d.disabled {
                // we don't have a WANIP or WANPPP url for this device, ask for it
                self.connect(di, registry, now);
            }
        }
    }

    // --- HTTP ---------------------------------------------------------------------------------

    fn start_request(
        &mut self,
        di: usize,
        kind: Request,
        timeout: Duration,
        registry: &Registry,
        now: Instant,
    ) {
        let d = &mut self.devices[di];
        if let Some(mut c) = d.conn.take() {
            let _ = registry.deregister(&mut c.stream);
        }
        let Ok(ip) = d.hostname.parse::<IpAddr>() else {
            // libtorrent resolves host names here; routers announce IP literals
            self.log.push(format!("device host {} is not an IP address; not resolved", d.hostname));
            d.disabled = true;
            return;
        };
        let port = d.port;
        let token = Token(self.http_token_base + di);
        let connected = TcpStream::connect(SocketAddr::new(ip, port)).and_then(|mut s| {
            registry.register(&mut s, token, Interest::READABLE | Interest::WRITABLE)?;
            Ok(s)
        });
        match connected {
            Ok(stream) => {
                d.conn = Some(HttpConn {
                    stream,
                    kind,
                    request: Vec::new(),
                    written: 0,
                    buf: Vec::new(),
                    connected: false,
                    deadline: now + timeout,
                });
            }
            Err(e) => self.on_http_result(di, kind, Err(HttpError::Failed(e.to_string())), registry, now),
        }
    }

    /// The request for a connection that is up: its kind decides the body.
    fn build_request(&self, di: usize, kind: Request, local: IpAddr) -> Vec<u8> {
        let d = &self.devices[di];
        match kind {
            Request::Xml => {
                let url = http::Url {
                    scheme: "http".into(),
                    host: d.hostname.clone(),
                    port: d.port,
                    path: d.path.clone(),
                };
                http::get_request(&url, &self.settings.user_agent)
            }
            Request::GetIp => {
                let soap = upnp::get_external_ip_address(&d.service_namespace);
                upnp::post_request(
                    &d.path,
                    &d.hostname,
                    d.port,
                    &d.service_namespace,
                    upnp::ACTION_GET_IP,
                    &soap,
                )
            }
            Request::Map(i) => {
                let m = &d.mapping[i];
                let lease = if d.use_lease_duration { self.settings.lease_duration } else { 0 };
                let soap = upnp::add_port_mapping(
                    &d.service_namespace,
                    m.external_port,
                    m.protocol.unwrap_or(Protocol::Tcp),
                    m.local_port,
                    &local.to_string(),
                    &self.settings.user_agent,
                    lease,
                );
                upnp::post_request(
                    &d.path,
                    &d.hostname,
                    d.port,
                    &d.service_namespace,
                    upnp::ACTION_ADD,
                    &soap,
                )
            }
            Request::Unmap(i) => {
                let m = &d.mapping[i];
                let soap = upnp::delete_port_mapping(
                    &d.service_namespace,
                    m.external_port,
                    m.protocol.unwrap_or(Protocol::Tcp),
                );
                upnp::post_request(
                    &d.path,
                    &d.hostname,
                    d.port,
                    &d.service_namespace,
                    upnp::ACTION_DELETE,
                    &soap,
                )
            }
        }
    }

    /// A reactor event on device `di`'s HTTP connection.
    pub fn on_http_event(&mut self, di: usize, registry: &Registry, now: Instant) {
        let Some(d) = self.devices.get_mut(di) else { return };
        let Some(c) = d.conn.as_mut() else { return };
        if !c.connected {
            match (c.stream.take_error(), c.stream.peer_addr()) {
                (Ok(None), Ok(_)) => c.connected = true,
                (Ok(None), Err(e)) if e.kind() == io::ErrorKind::NotConnected => return,
                (Ok(Some(e)), _) | (Err(e), _) => {
                    return self.finish_request(di, Err(HttpError::Failed(e.to_string())), registry, now);
                }
                _ => {
                    return self.finish_request(
                        di,
                        Err(HttpError::Failed("connect failed".into())),
                        registry,
                        now,
                    );
                }
            }
        }
        if c.request.is_empty() {
            let kind = c.kind;
            let local = c.stream.local_addr().map(|a| a.ip()).unwrap_or(self.listen_address.into());
            let req = self.build_request(di, kind, local);
            let c = self.devices[di].conn.as_mut().expect("still there");
            c.request = req;
            if kind != Request::Xml {
                self.log.push(format!("sending: {}", String::from_utf8_lossy(&c.request)));
            }
        }
        let c = self.devices[di].conn.as_mut().expect("still there");
        while c.written < c.request.len() {
            match c.stream.write(&c.request[c.written..]) {
                Ok(n) => c.written += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    return self.finish_request(di, Err(HttpError::Failed(e.to_string())), registry, now);
                }
            }
        }
        let mut eof = false;
        let mut chunk = [0u8; 4096];
        loop {
            match c.stream.read(&mut chunk) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => {
                    c.buf.extend_from_slice(&chunk[..n]);
                    if c.buf.len() > http::MAX_RESPONSE {
                        return self.finish_request(
                            di,
                            Err(HttpError::Failed("response too large".into())),
                            registry,
                            now,
                        );
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    return self.finish_request(di, Err(HttpError::Failed(e.to_string())), registry, now);
                }
            }
        }
        match http::parse_response(&c.buf, eof) {
            Ok(resp) => self.finish_request(di, Ok(resp), registry, now),
            Err(http::ParseError::Incomplete) => {}
            Err(http::ParseError::Invalid("truncated HTTP header")) => {
                self.finish_request(di, Err(HttpError::Incomplete), registry, now)
            }
            Err(http::ParseError::Invalid(m)) => {
                self.finish_request(di, Err(HttpError::Failed(m.into())), registry, now)
            }
        }
    }

    fn finish_request(
        &mut self,
        di: usize,
        result: Result<http::Response, HttpError>,
        registry: &Registry,
        now: Instant,
    ) {
        let Some(mut c) = self.devices[di].conn.take() else { return };
        let _ = registry.deregister(&mut c.stream);
        self.on_http_result(di, c.kind, result, registry, now);
    }

    fn on_http_result(
        &mut self,
        di: usize,
        kind: Request,
        result: Result<http::Response, HttpError>,
        registry: &Registry,
        now: Instant,
    ) {
        match kind {
            Request::Xml => self.on_upnp_xml(di, result, registry, now),
            Request::GetIp => self.on_upnp_get_ip_address_response(di, result, registry, now),
            Request::Map(i) => self.on_upnp_map_response(di, i, result, registry, now),
            Request::Unmap(i) => self.on_upnp_unmap_response(di, i, result, registry, now),
        }
    }

    // --- Responses ----------------------------------------------------------------------------

    fn on_upnp_xml(
        &mut self,
        di: usize,
        result: Result<http::Response, HttpError>,
        registry: &Registry,
        now: Instant,
    ) {
        if self.closing {
            return;
        }
        let url = self.devices[di].url.clone();
        let resp = match result {
            Err(HttpError::Failed(e)) => {
                self.log.push(format!("error while fetching control url from: {url}: {e}"));
                self.devices[di].disabled = true;
                return;
            }
            Err(HttpError::Incomplete) => {
                self.log
                    .push(format!("error while fetching control url from: {url}: incomplete HTTP message"));
                self.devices[di].disabled = true;
                return;
            }
            Ok(r) if r.status != 200 => {
                self.log.push(format!(
                    "error while fetching control url from: {url}: {} {}",
                    r.status, r.message
                ));
                self.devices[di].disabled = true;
                return;
            }
            Ok(r) => r,
        };
        let s = upnp::parse_device_description(&String::from_utf8_lossy(&resp.body));
        if s.control_url.is_empty() {
            self.log.push(format!("could not find a port mapping interface in response from: {url}"));
            self.devices[di].disabled = true;
            return;
        }
        let d = &mut self.devices[di];
        d.service_namespace = s.service_type;
        if !s.model.is_empty() {
            self.log.push(format!("router model: {}", s.model));
            self.model = s.model;
        }
        let mut control_url = if !s.url_base.is_empty() && !s.control_url.starts_with("http://") {
            // avoid double slashes in path
            let mut base = s.url_base.clone();
            if base.ends_with('/') && s.control_url.starts_with('/') {
                base.pop();
            }
            base + &s.control_url
        } else {
            s.control_url.clone()
        };
        if control_url.starts_with('/') {
            if let Ok(p) = crate::util::string::parse_url_components(&d.url) {
                let port = p.port.unwrap_or(if p.protocol.eq_ignore_ascii_case("http") { 80 } else { 443 });
                control_url = format!("{}://{}:{}{}", p.protocol, p.host, port, s.control_url);
            }
        }
        d.control_url = control_url;
        self.log.push(format!(
            "found control URL: {} namespace {} urlbase: {} in response from {url}",
            d.control_url, d.service_namespace, s.url_base
        ));
        match crate::util::string::parse_url_components(&d.control_url) {
            Ok(p) if p.protocol.eq_ignore_ascii_case("http") => {
                d.hostname = p.host;
                d.port = p.port.unwrap_or(80);
                d.path = if p.path.is_empty() { "/".into() } else { p.path };
            }
            Ok(p) => {
                self.log.push(format!("unsupported control URL protocol '{}'", p.protocol));
                d.disabled = true;
                return;
            }
            Err(e) => {
                self.log.push(format!("failed to parse URL '{}': {e:?}", d.control_url));
                d.disabled = true;
                return;
            }
        }
        self.log.push("getting external IP address".into());
        self.start_request(di, Request::GetIp, SOAP_TIMEOUT, registry, now);
    }

    fn on_upnp_get_ip_address_response(
        &mut self,
        di: usize,
        result: Result<http::Response, HttpError>,
        registry: &Registry,
        now: Instant,
    ) {
        if self.closing {
            return;
        }
        let resp = match result {
            Err(HttpError::Failed(e)) => {
                self.log.push(format!("error while getting external IP address: {e}"));
                self.map_first(di, registry, now);
                return;
            }
            Err(HttpError::Incomplete) => {
                self.log.push("error while getting external IP address: incomplete http message".into());
                self.map_first(di, registry, now);
                return;
            }
            Ok(r) if r.status != 200 => {
                self.log.push(format!("error while getting external IP address: {} {}", r.status, r.message));
                self.map_first(di, registry, now);
                return;
            }
            Ok(r) => r,
        };
        let body = String::from_utf8_lossy(&resp.body);
        self.log.push(format!("get external IP address response: {body}"));
        let s = upnp::parse_external_address(&body);
        if s.error.error_code != -1 {
            self.log.push(format!("error while getting external IP address, code: {}", s.error.error_code));
        }
        if !s.ip_address.is_empty() {
            self.log.push(format!("got router external IP address {}", s.ip_address));
            self.devices[di].external_ip = s.ip_address.trim().parse().ok();
        } else {
            self.log.push("failed to find external IP address in response".into());
        }
        self.map_first(di, registry, now);
    }

    fn map_first(&mut self, di: usize, registry: &Registry, now: Instant) {
        if !self.mappings.is_empty() {
            self.update_map(di, 0, registry, now);
        }
    }

    fn on_upnp_map_response(
        &mut self,
        di: usize,
        mapping: usize,
        result: Result<http::Response, HttpError>,
        registry: &Registry,
        now: Instant,
    ) {
        let resp = match result {
            Err(HttpError::Failed(e)) => {
                self.log.push(format!("error while adding port map: {e}"));
                self.devices[di].disabled = true;
                return;
            }
            Err(HttpError::Incomplete) => {
                self.log.push("error while adding port map: incomplete http message".into());
                self.next(di, mapping, registry, now);
                return;
            }
            Ok(r) => r,
        };
        if self.closing {
            return;
        }
        if !upnp::is_xml_content_type(resp.header("content-type")) {
            self.log.push(format!(
                "error while adding port map: invalid content-type, \"{}\". Expected text/xml or application/soap+xml",
                resp.header("content-type").unwrap_or_default()
            ));
            self.next(di, mapping, registry, now);
            return;
        }
        // We don't want to ignore responses with return codes other than 200 since those
        // might contain valid UPnP error codes
        let body = String::from_utf8_lossy(&resp.body).into_owned();
        let code = upnp::parse_error_code(&body);
        if code != -1 {
            self.log.push(format!("error while adding port map, code: {code}"));
        }
        if mapping >= self.devices[di].mapping.len() {
            return;
        }
        if code == upnp::ERR_ONLY_PERMANENT_LEASES_SUPPORTED {
            // The gateway only supports permanent leases
            let d = &mut self.devices[di];
            d.use_lease_duration = false;
            d.mapping[mapping].act = Action::Add;
            d.mapping[mapping].failcount += 1;
            self.update_map(di, mapping, registry, now);
            return;
        } else if code == upnp::ERR_EXTERNAL_PORT_MUST_BE_WILDCARD {
            self.return_error(mapping, code);
        } else if (code == upnp::ERR_PORT_MAPPING_CONFLICT || code == upnp::ERR_ACTION_FAILED)
            && self.devices[di].mapping[mapping].failcount < 4
        {
            // some routers return 501 action failed, instead of 716. The external port
            // conflicts with another mapping: pick a random port
            let m = &mut self.devices[di].mapping[mapping];
            m.external_port = 40000 + crate::util::random::random_below(10000) as u16;
            m.act = Action::Add;
            m.failcount += 1;
            self.update_map(di, mapping, registry, now);
            return;
        } else if code != -1 {
            self.return_error(mapping, code);
        }
        self.log.push(format!("map response: {body}"));
        if code == -1 {
            let d = &mut self.devices[di];
            let m = &mut d.mapping[mapping];
            self.events.push(PortmapEvent {
                mapping,
                external_ip: d.external_ip,
                external_port: m.external_port,
                protocol: m.protocol.unwrap_or(Protocol::Tcp),
                error: None,
            });
            if d.use_lease_duration && self.settings.lease_duration != 0 {
                let expires = now + Duration::from_secs(self.settings.lease_duration as u64 * 3 / 4);
                m.expires = Some(expires);
                if self.refresh_at.is_none_or(|r| r < now || r > expires) {
                    self.refresh_at = Some(expires);
                }
            } else {
                m.expires = None;
            }
            m.failcount = 0;
        }
        self.next(di, mapping, registry, now);
    }

    fn return_error(&mut self, mapping: usize, code: i32) {
        let proto = self.mappings.get(mapping).and_then(|m| m.protocol).unwrap_or(Protocol::Tcp);
        self.events.push(PortmapEvent {
            mapping,
            external_ip: None,
            external_port: 0,
            protocol: proto,
            error: Some(upnp::mapping_error(code)),
        });
    }

    fn on_upnp_unmap_response(
        &mut self,
        di: usize,
        mapping: usize,
        result: Result<http::Response, HttpError>,
        registry: &Registry,
        now: Instant,
    ) {
        let error = match &result {
            Err(HttpError::Failed(e)) => {
                self.log.push(format!("error while deleting portmap: {e}"));
                Some(e.clone())
            }
            Err(HttpError::Incomplete) => {
                self.log.push("error while deleting portmap: incomplete http message".into());
                Some("incomplete http message".into())
            }
            Ok(r) if r.status != 200 => {
                self.log.push(format!("error while deleting portmap: {} {}", r.status, r.message));
                Some(format!("HTTP {} {}", r.status, r.message))
            }
            Ok(r) => {
                let body = String::from_utf8_lossy(&r.body);
                self.log.push(format!("unmap response: {body}"));
                let code = upnp::parse_error_code(&body);
                (code != -1).then(|| upnp::mapping_error(code))
            }
        };
        let proto = self.mappings.get(mapping).and_then(|m| m.protocol).unwrap_or(Protocol::Tcp);
        self.events.push(PortmapEvent {
            mapping,
            external_ip: None,
            external_port: 0,
            protocol: proto,
            error,
        });
        if let Some(m) = self.devices[di].mapping.get_mut(mapping) {
            m.protocol = None;
        }
        // free the slot in global mappings
        let all_gone =
            self.devices.iter().all(|d| d.mapping.get(mapping).is_none_or(|m| m.protocol.is_none()));
        if all_gone {
            if let Some(g) = self.mappings.get_mut(mapping) {
                g.protocol = None;
            }
        }
        self.next(di, mapping, registry, now);
    }

    // --- Mapping state ------------------------------------------------------------------------

    fn next(&mut self, di: usize, i: usize, registry: &Registry, now: Instant) {
        if self.devices[di].disabled {
            return;
        }
        if i + 1 < self.mappings.len() {
            self.update_map(di, i + 1, registry, now);
        } else if let Some(j) = self.devices[di].mapping.iter().position(|m| m.act != Action::None) {
            self.update_map(di, j, registry, now);
        }
    }

    fn update_map(&mut self, di: usize, i: usize, registry: &Registry, now: Instant) {
        let d = &mut self.devices[di];
        debug_assert!(!d.disabled);
        if d.conn.is_some() {
            return;
        }
        // this should not happen, but in case it does, don't fail at runtime
        if i >= d.mapping.len() {
            return;
        }
        let m = &mut d.mapping[i];
        if m.act == Action::None || m.protocol.is_none() {
            self.log.push(format!("mapping {i} does not need updating, skipping"));
            m.act = Action::None;
            self.next(di, i, registry, now);
            return;
        }
        debug_assert!(!d.service_namespace.is_empty());
        let act = m.act;
        self.log.push(format!("connecting to {}", d.hostname));
        if act == Action::Add {
            if m.failcount > MAX_FAILCOUNT {
                m.act = Action::None;
                // giving up
                self.next(di, i, registry, now);
                return;
            }
            self.start_request(di, Request::Map(i), SOAP_TIMEOUT, registry, now);
        } else if act == Action::Del {
            self.start_request(di, Request::Unmap(i), SOAP_TIMEOUT, registry, now);
        }
        if let Some(m) = self.devices[di].mapping.get_mut(i) {
            m.act = Action::None;
            m.expires = Some(now + PENDING_EXPIRY);
        }
    }

    /// libtorrent `on_expire`: refreshes the mappings whose lease is running out.
    fn on_expire(&mut self, registry: &Registry, now: Instant) {
        if self.closing {
            return;
        }
        let mut next_expire: Option<Instant> = None;
        for di in 0..self.devices.len() {
            if self.devices[di].disabled {
                continue;
            }
            for m in 0..self.mappings.len() {
                let Some(expires) = self.devices[di].mapping.get(m).and_then(|x| x.expires) else { continue };
                if expires <= now {
                    self.devices[di].mapping[m].act = Action::Add;
                    self.update_map(di, m, registry, now);
                }
                if let Some(e) = self.devices[di].mapping.get(m).and_then(|x| x.expires) {
                    next_expire = Some(next_expire.map_or(e, |n| n.min(e)));
                }
            }
        }
        self.refresh_at = next_expire;
    }

    fn disable(&mut self, error: String) {
        self.disabled = true;
        // kill all mappings
        for i in 0..self.mappings.len() {
            let Some(proto) = self.mappings[i].protocol.take() else { continue };
            self.events.push(PortmapEvent {
                mapping: i,
                external_ip: None,
                external_port: 0,
                protocol: proto,
                error: Some(error.clone()),
            });
        }
        // we cannot clear the devices since there might be outstanding requests relying on
        // the device entry being present when they complete
        self.broadcast_at = None;
        self.refresh_at = None;
        self.map_at = None;
        self.unicast = None;
        self.multicast = None;
    }

    /// Deletes the mappings on every router that has them (the requests go out over HTTP, so
    /// the mapper stays alive until `pending` clears) and stops listening.
    pub fn close(&mut self, registry: &Registry, now: Instant) {
        self.refresh_at = None;
        self.broadcast_at = None;
        self.map_at = None;
        self.closing = true;
        self.unicast = None;
        self.multicast = None;
        for di in 0..self.devices.len() {
            let d = &mut self.devices[di];
            if d.disabled || d.control_url.is_empty() {
                continue;
            }
            for (i, m) in d.mapping.iter_mut().enumerate() {
                if m.protocol.is_none() {
                    continue;
                }
                if m.act == Action::Add {
                    m.act = Action::None;
                    continue;
                }
                m.act = Action::Del;
                if let Some(g) = self.mappings.get_mut(i) {
                    g.protocol = None;
                }
            }
            self.map_first(di, registry, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mio::{Events, Poll};
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    const ROOT_XML: &str = "<root><specVersion><major>1</major><minor>0</minor></specVersion>\
<URLBase>http://127.0.0.1:{port}</URLBase><device>\
<deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>\
<friendlyName>D-Link Router</friendlyName><modelName>D-Link Router</modelName>\
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>\
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>\
<serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>\
<serviceId>urn:upnp-org:serviceId:WANIPConnection</serviceId><controlURL>/WANIPConnection</controlURL>\
<eventSubURL>/WANIPConnection</eventSubURL><SCPDURL>/WANIPConnection.xml</SCPDURL>\
</service></serviceList></device></deviceList></device></deviceList></device></root>";

    const SOAP_ADD: &str = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body><u:AddPortMappingResponse xmlns:u=\"urn:schemas-upnp-org:service:WANIPConnection:1\">\
</u:AddPortMappingResponse></s:Body></s:Envelope>";
    const SOAP_DELETE: &str = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body><u:DeletePortMappingResponse xmlns:u=\"urn:schemas-upnp-org:service:WANIPConnection:1\">\
</u:DeletePortMappingResponse></s:Body></s:Envelope>";
    const SOAP_IP: &str = "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
<u:GetExternalIPAddressResponse xmlns:u=\"urn:schemas-upnp-org:service:WANIPConnection:1\">\
<NewExternalIPAddress>123.10.20.30</NewExternalIPAddress></u:GetExternalIPAddressResponse>\
</s:Body></s:Envelope>";
    const SOAP_CONFLICT: &str = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">\
<s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail>\
<UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>718</errorCode>\
<errorDescription>ConflictInMappingEntry</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>";

    /// What the fake router saw: one entry per SOAP action, with the external port asked.
    type Seen = Arc<Mutex<Vec<(String, u16)>>>;

    /// Stops the fake router when dropped and waits for both of its threads, so it cannot
    /// answer a later test's search. (Before it joined, a responder with up to 100 ms left and
    /// an HTTP server that never stopped made the next test find two routers: CI runs
    /// 38913226275 and 38913226650.)
    struct Stop {
        flag: Arc<std::sync::atomic::AtomicBool>,
        http_port: u16,
        threads: Vec<std::thread::JoinHandle<()>>,
    }

    impl Drop for Stop {
        fn drop(&mut self) {
            self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
            // wake the HTTP thread out of accept(); it sees the flag and exits
            let _ = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, self.http_port));
            for t in self.threads.drain(..) {
                let _ = t.join();
            }
        }
    }

    /// The IGD tests share the SSDP port: one at a time.
    static IGD_LOCK: Mutex<()> = Mutex::new(());

    /// A fake IGD: an HTTP server for the description and the SOAP calls, and an SSDP responder
    /// on the standard port that points at it. `conflicts` makes the first AddPortMapping of
    /// each port fail with 718, as a router with the port taken does.
    fn fake_igd(conflicts: bool) -> (Seen, Stop) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let http = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = http.local_addr().unwrap().port();
        let seen2 = seen.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        let http_thread = std::thread::spawn(move || {
            let conflicted = Mutex::new(Vec::<u16>::new());
            for stream in http.incoming() {
                let Ok(mut stream) = stream else { break };
                if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let mut reader = io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
                let mut len = 0;
                let mut action = String::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    if let Some(v) = h.strip_prefix("Content-Length: ") {
                        len = v.parse().unwrap();
                    }
                    if let Some(v) = h.strip_prefix("Soapaction: ") {
                        action = v.split('#').nth(1).unwrap_or("").trim_matches('"').to_owned();
                    }
                }
                let mut body = vec![0u8; len];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8_lossy(&body).into_owned();
                let ext_port: u16 = body
                    .split("<NewExternalPort>")
                    .nth(1)
                    .and_then(|s| s.split('<').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let (status, resp) = match (path.as_str(), action.as_str()) {
                    ("/upnp.xml", _) => ("200 OK", ROOT_XML.replace("{port}", &port.to_string())),
                    ("/WANIPConnection", "GetExternalIPAddress") => ("200 OK", SOAP_IP.to_owned()),
                    ("/WANIPConnection", "AddPortMapping") => {
                        let mut c = conflicted.lock().unwrap();
                        // the ports first asked for are taken; the random retry ports are free
                        if conflicts && ext_port < 40000 && !c.contains(&ext_port) {
                            c.push(ext_port);
                            ("500 Internal Server Error", SOAP_CONFLICT.to_owned())
                        } else {
                            ("200 OK", SOAP_ADD.to_owned())
                        }
                    }
                    ("/WANIPConnection", "DeletePortMapping") => ("200 OK", SOAP_DELETE.to_owned()),
                    _ => ("404 Not Found", String::new()),
                };
                if !action.is_empty() {
                    seen2.lock().unwrap().push((action.clone(), ext_port));
                }
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/xml; charset=\"utf-8\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp}",
                    resp.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        // the SSDP responder: a socket on the SSDP port, in the group on loopback
        let ssdp =
            sys::udp_socket(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), upnp::SSDP_PORT), true).unwrap();
        ssdp.join_multicast_v4(&upnp::SSDP_MULTICAST, &Ipv4Addr::LOCALHOST).unwrap();
        let stopped = stop.clone();
        let ssdp_thread = std::thread::spawn(move || {
            let responder = ssdp;
            let mut buf = [0u8; 1500];
            responder.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok((n, from)) = responder.recv_from(&mut buf) else { continue };
                if !buf[..n].starts_with(b"M-SEARCH") || !from.ip().is_loopback() {
                    continue;
                }
                let msg = format!(
                    "HTTP/1.1 200 OK\r\nST:upnp:rootdevice\r\nUSN:uuid:000f-66d6-7296000099dc::upnp:rootdevice\r\n\
Location: http://127.0.0.1:{port}/upnp.xml\r\nServer: Custom/1.0 UPnP/1.0 Proc/Ver\r\nEXT:\r\n\
Cache-Control:max-age=180\r\nDATE: Fri, 02 Jan 1970 08:10:38 GMT\r\n\r\n"
                );
                let _ = responder.send_to(msg.as_bytes(), from);
            }
        });
        (seen, Stop { flag: stop, http_port: port, threads: vec![http_thread, ssdp_thread] })
    }

    struct Harness {
        poll: Poll,
        events: Events,
        upnp: Upnp,
        got: Vec<PortmapEvent>,
    }

    impl Harness {
        fn new() -> Harness {
            let poll = Poll::new().unwrap();
            let settings = UpnpSettings {
                ignore_nonrouters: false,
                lease_duration: 3600,
                user_agent: "test agent".into(),
            };
            let mut upnp = Upnp::new(settings, Ipv4Addr::LOCALHOST, Ipv4Addr::new(255, 0, 0, 0), 1000);
            upnp.start(poll.registry(), Token(1), Token(2), Instant::now());
            Harness { poll, events: Events::with_capacity(16), upnp, got: Vec::new() }
        }

        /// Drives the mapper until `done` or the timeout.
        fn run_until(&mut self, secs: u64, mut done: impl FnMut(&Harness) -> bool) -> bool {
            let deadline = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < deadline {
                if done(self) {
                    return true;
                }
                let now = Instant::now();
                let timeout = self
                    .upnp
                    .deadline()
                    .map_or(Duration::from_millis(50), |d| d.saturating_duration_since(now));
                self.poll.poll(&mut self.events, Some(timeout.min(Duration::from_millis(50)))).unwrap();
                let now = Instant::now();
                let tokens: Vec<usize> = self.events.iter().map(|e| e.token().0).collect();
                for t in tokens {
                    match t {
                        1 => self.upnp.on_readable(true, self.poll.registry(), now),
                        2 => self.upnp.on_readable(false, self.poll.registry(), now),
                        t if self.upnp.owns_http_token(t) => self.upnp.on_http_event(
                            t - self.upnp.http_token_base(),
                            self.poll.registry(),
                            now,
                        ),
                        _ => {}
                    }
                }
                self.upnp.service(self.poll.registry(), now);
                self.got.append(&mut self.upnp.events);
            }
            done(self)
        }

        fn dump_log(&mut self) {
            for l in self.upnp.log.drain(..) {
                eprintln!("UPnP: {l}");
            }
        }
    }

    fn run(conflicts: bool) {
        let _one_at_a_time = IGD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (seen, _stop) = fake_igd(conflicts);
        let mut h = Harness::new();
        let found = h.run_until(8, |h| !h.upnp.router_model().is_empty());
        h.dump_log();
        assert!(found, "the fake router was not found");
        assert_eq!(h.upnp.router_model(), "D-Link Router");
        let now = Instant::now();
        let m1 = h.upnp.add_mapping(Protocol::Tcp, 500, 500, h.poll.registry(), now).unwrap();
        let m2 = h.upnp.add_mapping(Protocol::Udp, 501, 501, h.poll.registry(), now).unwrap();
        let mapped = h.run_until(10, |h| h.got.len() >= 2);
        h.dump_log();
        assert!(mapped, "{:?}", h.got);
        let ok = |h: &Harness, m: usize, port: u16| {
            h.got
                .iter()
                .filter(|e| {
                    e.mapping == m
                        && e.error.is_none()
                        && e.external_ip == Some("123.10.20.30".parse().unwrap())
                        && (if conflicts { e.external_port != port } else { e.external_port == port })
                })
                .count()
        };
        assert_eq!(ok(&h, m1, 500), 1, "{:?}", h.got);
        assert_eq!(ok(&h, m2, 501), 1, "{:?}", h.got);
        h.upnp.close(h.poll.registry(), Instant::now());
        let unmapped = h.run_until(10, |h| h.got.len() >= 4 && !h.upnp.pending());
        h.dump_log();
        assert!(unmapped, "{:?}", h.got);
        // there should have been two DeleteMapping calls
        let seen = seen.lock().unwrap().clone();
        let deletes: Vec<u16> =
            seen.iter().filter(|(a, _)| a == "DeletePortMapping").map(|(_, p)| *p).collect();
        assert_eq!(deletes.len(), 2, "{seen:?}");
        if conflicts {
            // the first try of each port was refused, the retry used a random high port, and
            // that is the port deleted
            assert_eq!(seen.iter().filter(|(a, _)| a == "AddPortMapping").count(), 4, "{seen:?}");
            assert!(deletes.iter().all(|p| (40000..50000).contains(p)), "{deletes:?}");
        } else {
            assert_eq!(deletes, [500, 501]);
        }
    }

    #[test]
    fn maps_and_unmaps_on_a_loopback_igd() {
        run(false);
    }

    #[test]
    fn a_port_conflict_moves_to_a_random_port() {
        run(true);
    }

    #[test]
    fn max_mappings() {
        let settings =
            UpnpSettings { ignore_nonrouters: false, lease_duration: 3600, user_agent: "t".into() };
        let poll = Poll::new().unwrap();
        let mut upnp = Upnp::new(settings, Ipv4Addr::LOCALHOST, Ipv4Addr::new(255, 0, 0, 0), 1000);
        let now = Instant::now();
        for i in 0..50u16 {
            assert!(upnp.add_mapping(Protocol::Tcp, 500 + i, 500 + i, poll.registry(), now).is_some());
        }
        assert!(
            upnp.add_mapping(Protocol::Tcp, 600, 600, poll.registry(), now).is_none(),
            "the 51st is refused"
        );
    }

    #[test]
    fn no_router_disables_the_mapper() {
        let settings =
            UpnpSettings { ignore_nonrouters: false, lease_duration: 3600, user_agent: "t".into() };
        let poll = Poll::new().unwrap();
        let mut upnp = Upnp::new(settings, Ipv4Addr::LOCALHOST, Ipv4Addr::new(255, 0, 0, 0), 1000);
        let mut now = Instant::now();
        upnp.start(poll.registry(), Token(1), Token(2), now);
        upnp.add_mapping(Protocol::Tcp, 500, 500, poll.registry(), now).unwrap();
        // twelve searches 2, 4, ... 24 s apart, then "no router found"
        let mut searches = 1;
        while let Some(d) = upnp.broadcast_at {
            now = d;
            upnp.service(poll.registry(), now);
            searches += 1;
            assert!(searches < 20);
        }
        assert_eq!(searches, MAX_SEARCHES as usize + 1, "twelve searches, then the timer that gives up");
        assert!(upnp.disabled);
        assert_eq!(upnp.events.len(), 1);
        assert_eq!(upnp.events[0].error.as_deref(), Some("no router found"));
        assert!(upnp.add_mapping(Protocol::Tcp, 501, 501, poll.registry(), now).is_none());
    }
}
