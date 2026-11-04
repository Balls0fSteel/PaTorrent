//! Talking to the running search helper on `127.0.0.1`: a tiny HTTP/1.1 client of its own
//! (the shared fetch only `GET`s; this one also `POST`s and `DELETE`s, and it only ever
//! connects to the loopback port the helper was given), and Jackett's admin API.
//!
//! What it calls, read from Jackett's source (`github.com/Jackett/Jackett`, master,
//! 2026-11-04):
//!
//! - `GET /health` (`src/Jackett.Server/Controllers/HealthcheckController.cs`, anonymous):
//!   `{"status":"OK"}`.
//! - `POST /UI/Dashboard` with the form field `password` (`UIController.cs`): signs in and sets
//!   the `Jackett` cookie (cookie authentication, `src/Jackett.Server/Startup.cs`; the admin
//!   API takes no API key).
//! - `GET /api/v2.0/indexers?configured=false` (`IndexerApiController.cs`): every indexer,
//!   as `src/Jackett.Common/Models/DTO/Indexer.cs` (`id`, `name`, `description`, `type`,
//!   `configured`, `language`, `last_error`, ...).
//! - `GET` / `POST /api/v2.0/indexers/{id}/Config`: the indexer's settings as a JSON array of
//!   items; posting them back (`src/Jackett.Common/Models/DTO/Config.cs`) configures it
//!   (204), a public indexer with its defaults.
//! - `DELETE /api/v2.0/indexers/{id}`: unconfigures it.
//! - `POST /api/v2.0/server/Shutdown?apikey=` (`ServerConfigurationController.cs`, anonymous
//!   with the API key): a clean stop.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use super::json::{self, Json};

/// One request's time limit (connect, send, every read).
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// The largest answer read (the indexer listing is about 1 MB).
const MAX_ANSWER: usize = 32 << 20;

/// An answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn ok(self, what: &str) -> Result<Answer, String> {
        if (200..300).contains(&self.status) {
            Ok(self)
        } else {
            let text = String::from_utf8_lossy(&self.body);
            let text: String = text.chars().take(200).collect();
            Err(format!("{what}: HTTP {} {}", self.status, text.trim()))
        }
    }
}

/// Sends one request to `127.0.0.1:port` and reads the whole answer (`Connection: close`;
/// `Content-Length`, chunked or until the end).
pub fn request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    timeout: Duration,
) -> Result<Answer, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: {}\r\nConnection: close\r\n",
        patorrent_core::USER_AGENT
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {}\r\n", v.replace(['\r', '\n'], " ")));
    }
    if !body.is_empty() || method == "POST" {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.take(MAX_ANSWER as u64 + 1).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    if raw.len() > MAX_ANSWER {
        return Err("answer too large".to_owned());
    }
    parse_answer(&raw)
}

/// An HTTP/1.x answer read to its end.
pub fn parse_answer(raw: &[u8]) -> Result<Answer, String> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("a malformed answer")?;
    let head = String::from_utf8_lossy(&raw[..end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|_| status_line.starts_with("HTTP/1."))
        .ok_or("a malformed answer")?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let mut a = Answer { status, headers, body: Vec::new() };
    let rest = &raw[end + 4..];
    a.body = if a.header("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked")) {
        dechunk(rest)?
    } else if let Some(n) = a.header("content-length").and_then(|n| n.parse::<usize>().ok()) {
        rest.get(..n).ok_or("the answer ended early")?.to_vec()
    } else {
        rest.to_vec()
    };
    Ok(a)
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|w| w == b"\r\n").ok_or("a malformed chunk")?;
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| "a malformed chunk".to_owned())?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        out.extend_from_slice(data.get(..size).ok_or("the answer ended early")?);
        data = data.get(size + 2..).unwrap_or(&[]);
    }
}

/// `application/x-www-form-urlencoded` for one value.
fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(b as char),
            b' ' => out.push('+'),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Whether the helper answers its health check.
pub fn healthy(port: u16, timeout: Duration) -> bool {
    request(port, "GET", "/health", &[], &[], timeout).is_ok_and(|a| {
        a.status == 200
            && json::parse(&String::from_utf8_lossy(&a.body)).is_ok_and(|v| v.str("status") == Some("OK"))
    })
}

/// One of the helper's indexers, as its listing has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Indexer {
    pub id: String,
    pub name: String,
    pub description: String,
    /// `public`, `semi-private` or `private`.
    pub kind: String,
    pub language: String,
    pub configured: bool,
    pub last_error: String,
    /// Behind Cloudflare: Jackett needs FlareSolverr, which the helper does not include
    /// ([`needs_flaresolverr`]).
    pub needs_flaresolverr: bool,
}

impl Indexer {
    pub fn is_public(&self) -> bool {
        self.kind.eq_ignore_ascii_case("public")
    }
}

/// The indexers in the listing's JSON (`text`), in its order.
pub fn parse_indexers(text: &str) -> Result<Vec<Indexer>, String> {
    let v = json::parse(text)?;
    let Json::Array(items) = v else { return Err("the indexer listing is not a list".to_owned()) };
    Ok(items
        .iter()
        .filter_map(|i| {
            let id = i.str("id").filter(|s| !s.is_empty())?.to_owned();
            Some(Indexer {
                name: i.str("name").unwrap_or(&id).to_owned(),
                description: i.str("description").unwrap_or("").to_owned(),
                kind: i.str("type").unwrap_or("").to_owned(),
                language: i.str("language").unwrap_or("").to_owned(),
                configured: i.get("configured").and_then(Json::as_bool).unwrap_or(false),
                last_error: i.str("last_error").unwrap_or("").to_owned(),
                needs_flaresolverr: false,
                id,
            })
        })
        .collect())
}

/// Whether an indexer's Cardigann definition (`Definitions/<id>.yml` in the release) has the
/// `info_flaresolverr` setting, which Jackett shows as "This site may use Cloudflare DDoS
/// Protection, therefore Jackett requires FlareSolverr to access it"
/// (`src/Jackett.Common/Indexers/Definitions/CardigannIndexer.cs`).
pub fn needs_flaresolverr(definition: &str) -> bool {
    definition.lines().any(|l| {
        let l = l.trim();
        l.strip_prefix("type:").is_some_and(|t| t.trim().trim_matches(['"', '\'']) == "info_flaresolverr")
    })
}

/// A signed-in session with the helper's admin API.
#[derive(Clone, Debug)]
pub struct Admin {
    pub port: u16,
    cookie: String,
}

impl Admin {
    /// Signs in with the admin password: `POST /UI/Dashboard`, then the `Set-Cookie` answers.
    pub fn login(port: u16, password: &str) -> Result<Admin, String> {
        let body = format!("password={}", form_encode(password));
        let a = request(
            port,
            "POST",
            "/UI/Dashboard",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            body.as_bytes(),
            TIMEOUT,
        )?;
        let cookie: Vec<String> = a
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .filter_map(|(_, v)| v.split(';').next())
            .map(|c| c.trim().to_owned())
            .filter(|c| c.contains('=') && !c.ends_with('='))
            .collect();
        if cookie.is_empty() {
            return Err(format!("the search helper did not sign us in (HTTP {})", a.status));
        }
        Ok(Admin { port, cookie: cookie.join("; ") })
    }

    fn call(&self, method: &str, path: &str, body: Option<&Json>) -> Result<Answer, String> {
        let text = body.map(Json::to_text).unwrap_or_default();
        let mut headers = vec![("Cookie", self.cookie.as_str()), ("Accept", "application/json")];
        if body.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        let a = request(self.port, method, path, &headers, text.as_bytes(), TIMEOUT)?;
        // signed out: a redirect to the login page (or 401)
        if (300..400).contains(&a.status) || a.status == 401 {
            return Err("the search helper refused our password".to_owned());
        }
        a.ok(path)
    }

    /// Every indexer the helper has (`?configured=false`).
    pub fn indexers(&self) -> Result<Vec<Indexer>, String> {
        let a = self.call("GET", "/api/v2.0/indexers?configured=false", None)?;
        parse_indexers(&String::from_utf8_lossy(&a.body))
    }

    /// Configures indexer `id` with its default settings: its config items as the helper hands
    /// them out, posted back.
    pub fn configure(&self, id: &str) -> Result<(), String> {
        let id = path_segment(id)?;
        let path = format!("/api/v2.0/indexers/{id}/Config");
        let a = self.call("GET", &path, None)?;
        let items = json::parse(&String::from_utf8_lossy(&a.body))?;
        if !matches!(items, Json::Array(_)) {
            return Err(format!("{id}: its settings are not a list"));
        }
        self.call("POST", &path, Some(&items)).map(drop)
    }

    /// Unconfigures indexer `id`.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let id = path_segment(id)?;
        self.call("DELETE", &format!("/api/v2.0/indexers/{id}"), None).map(drop)
    }
}

/// An indexer id as a URL path segment: Jackett's are `[a-z0-9-]`; anything else is refused.
fn path_segment(id: &str) -> Result<&str, String> {
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        Ok(id)
    } else {
        Err(format!("refused: an indexer id {id:?}"))
    }
}

/// Asks the helper to stop cleanly (blocking, short).
pub fn shutdown(port: u16, api_key: &str) -> bool {
    let path = format!("/api/v2.0/server/Shutdown?apikey={}", form_encode(api_key));
    request(port, "POST", &path, &[], &[], Duration::from_secs(5)).is_ok_and(|a| a.status < 300)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_parse() {
        let a = parse_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX: y\r\n\r\nhello trailing").unwrap();
        assert_eq!((a.status, a.body.as_slice(), a.header("x")), (200, &b"hello"[..], Some("y")));
        let a = parse_answer(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5;x=1\r\npedia\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(a.body, b"Wikipedia");
        let a = parse_answer(b"HTTP/1.0 302 Found\r\nLocation: /UI/Login\r\n\r\n").unwrap();
        assert_eq!((a.status, a.body.len()), (302, 0));
        assert!(parse_answer(b"garbage").is_err());
        assert!(parse_answer(b"SSH-2.0\r\n\r\n").is_err());
        assert!(parse_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nshort").is_err());
        assert_eq!(form_encode("a b&c=d/é"), "a+b%26c%3Dd%2F%C3%A9");
    }

    #[test]
    fn indexers_and_flaresolverr() {
        let list = parse_indexers(
            r#"[{"id":"1337x","name":"1337x","description":"d","type":"public","configured":false,
                "site_link":"https://1337x.to/","language":"en-US","last_error":"","potatoenabled":true,
                "caps":[{"ID":"2000","Name":"Movies"}]},
               {"id":"privatehd","name":"PrivateHD","type":"private","configured":true,"language":"en-US"},
               {"name":"no id"}]"#,
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].is_public() && !list[0].configured);
        assert_eq!(list[0].language, "en-US");
        assert!(!list[1].is_public() && list[1].configured);
        assert!(parse_indexers("{}").is_err());
        assert!(needs_flaresolverr("settings:\n  - name: info_flaresolverr\n    type: info_flaresolverr\n"));
        assert!(!needs_flaresolverr("settings:\n  - name: info\n    type: info\n"));
        assert!(path_segment("../x").is_err());
        assert!(path_segment("1337x").is_ok());
    }
}
