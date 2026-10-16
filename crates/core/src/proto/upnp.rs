// Ported from libtorrent-rasterbar 2.0.11: src/upnp.cpp (discover_device_impl's M-SEARCH,
// on_reply's header checks, find_control_url, find_error_code, find_ip_address, the SOAP
// bodies of post / create_port_mapping / delete_port_mapping / get_ip_address, the error
// table), include/libtorrent/upnp.hpp (parse_state), test/test_xml.cpp (upnp_parser1-5)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: sans-IO; SSDP datagrams are read by a small header parser of their own instead of
// the general `http_parser`.

//! UPnP IGD (Internet Gateway Device) messages: SSDP discovery, the device description's
//! WANIPConnection / WANPPPConnection control URL, and the SOAP port-mapping calls.

use std::net::Ipv4Addr;

use crate::alert::Protocol;
use crate::proto::xml::{self, Token};

pub const SSDP_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const SSDP_PORT: u16 = 1900;

/// libtorrent `discover_device_impl`'s search, byte for byte.
pub const MSEARCH: &[u8] = b"M-SEARCH * HTTP/1.1\r\n\
HOST: 239.255.255.250:1900\r\n\
ST:upnp:rootdevice\r\n\
MAN:\"ssdp:discover\"\r\n\
MX:3\r\n\
\r\n\r\n";

/// The WAN services that map ports, in the spelling `find_control_url` accepts.
pub const WAN_SERVICES: [&str; 3] = [
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

/// An SSDP datagram's header: a `HTTP/1.1 200 OK` search response or a `NOTIFY` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ssdp {
    /// 0 for a request.
    pub status: u16,
    /// Lower-case; empty for a response.
    pub method: String,
    pub headers: Vec<(String, String)>,
}

impl Ssdp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Why a datagram is ignored (libtorrent `on_reply`'s log lines).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Malformed,
    /// Neither a 200 response nor a NOTIFY.
    Status,
    /// The header never ended.
    Incomplete,
    MissingLocation,
}

/// Reads an SSDP datagram the way libtorrent's `http_parser` plus `upnp::on_reply` do: the
/// first line is a status line or a request line, headers follow up to a blank line, and
/// only `200` responses and `NOTIFY` requests with a `Location` header are of interest.
pub fn parse_ssdp(buf: &[u8]) -> Result<(Ssdp, String), Invalid> {
    let text = String::from_utf8_lossy(buf);
    // only newline-terminated lines count; what follows the last one is not a line yet
    let mut lines =
        text.split_inclusive('\n').filter(|l| l.ends_with('\n')).map(|l| l.trim_end_matches(['\n', '\r']));
    let first = lines.next().ok_or(Invalid::Malformed)?;
    let mut words = first.split(' ').filter(|w| !w.is_empty());
    let (status, method) = match words.next() {
        Some(p) if p.starts_with("HTTP/") => {
            let s = words.next().and_then(|s| s.parse::<u16>().ok()).ok_or(Invalid::Malformed)?;
            (s, String::new())
        }
        Some(m)
            if words.clone().count() == 2 && words.clone().nth(1).is_some_and(|v| v.starts_with("HTTP/")) =>
        {
            (0, m.to_ascii_lowercase())
        }
        _ => return Err(Invalid::Malformed),
    };
    let mut headers = Vec::new();
    let mut finished = false;
    for line in lines {
        if line.is_empty() {
            finished = true;
            break;
        }
        match line.split_once(':') {
            Some((k, v)) => headers.push((k.trim().to_owned(), v.trim().to_owned())),
            // libtorrent's parser treats a colon-less line as the end of the header
            None => {
                finished = true;
                break;
            }
        }
    }
    let s = Ssdp { status, method, headers };
    if s.status != 200 && s.method != "notify" {
        return Err(Invalid::Status);
    }
    if !finished {
        return Err(Invalid::Incomplete);
    }
    let location = s.header("location").filter(|l| !l.is_empty()).ok_or(Invalid::MissingLocation)?.to_owned();
    Ok((s, location))
}

/// libtorrent `parse_state`: what `find_control_url` collects from a device description.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParseState {
    in_service: bool,
    tag_stack: Vec<String>,
    pub control_url: String,
    pub service_type: String,
    pub model: String,
    pub url_base: String,
}

impl ParseState {
    /// The two innermost open tags are `str1` then `str2` (case-insensitive).
    fn top_tags(&self, str1: &str, str2: &str) -> bool {
        let mut i = self.tag_stack.iter().rev();
        i.next().is_some_and(|t| t.eq_ignore_ascii_case(str2))
            && i.next().is_some_and(|t| t.eq_ignore_ascii_case(str1))
    }
}

/// libtorrent `find_control_url`: the first WAN(IP|PPP)Connection service's control URL, the
/// model name and the URL base.
pub fn find_control_url(token: Token<'_>, state: &mut ParseState) {
    match token {
        Token::StartTag(s) => state.tag_stack.push(s.to_owned()),
        Token::EndTag(_) => {
            if let Some(top) = state.tag_stack.pop() {
                if state.in_service && top.eq_ignore_ascii_case("service") {
                    state.in_service = false;
                }
            }
        }
        Token::String(s) => {
            let Some(top) = state.tag_stack.last() else { return };
            // default to the first (or only) control url in the router's listing
            if !state.in_service && state.top_tags("service", "servicetype") && state.service_type.is_empty()
            {
                if WAN_SERVICES.iter().any(|w| s.eq_ignore_ascii_case(w)) {
                    state.service_type = s.to_owned();
                    state.in_service = true;
                }
            } else if state.control_url.is_empty()
                && state.in_service
                && state.top_tags("service", "controlurl")
                && !s.is_empty()
            {
                state.control_url = s.to_owned();
            } else if state.model.is_empty() && state.top_tags("device", "modelname") {
                state.model = s.to_owned();
            } else if top.eq_ignore_ascii_case("urlbase") {
                state.url_base = s.to_owned();
            }
        }
        _ => {}
    }
}

/// libtorrent `error_code_parse_state`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorCodeState {
    in_error_code: bool,
    exit: bool,
    /// -1 when the reply holds no `<errorCode>`.
    pub error_code: i32,
}

impl ErrorCodeState {
    pub fn new() -> ErrorCodeState {
        ErrorCodeState { error_code: -1, ..Default::default() }
    }
}

/// C `atoi`: leading whitespace, a sign, digits; 0 otherwise.
fn atoi(s: &str) -> i32 {
    let s = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let v = digits.parse::<i64>().unwrap_or(if digits.is_empty() { 0 } else { i64::MAX });
    let v = if neg { -v } else { v };
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// libtorrent `find_error_code`: the `<errorCode>` of a SOAP fault.
pub fn find_error_code(token: Token<'_>, state: &mut ErrorCodeState) {
    if state.exit {
        return;
    }
    match token {
        Token::StartTag("errorCode") => state.in_error_code = true,
        Token::String(s) if state.in_error_code => {
            state.error_code = atoi(s);
            state.exit = true;
        }
        _ => {}
    }
}

/// libtorrent `ip_address_parse_state`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IpAddressState {
    pub error: ErrorCodeState,
    in_ip_address: bool,
    pub ip_address: String,
}

impl IpAddressState {
    pub fn new() -> IpAddressState {
        IpAddressState { error: ErrorCodeState::new(), ..Default::default() }
    }
}

/// libtorrent `find_ip_address`: the `<NewExternalIPAddress>` of a GetExternalIPAddress reply,
/// or its error code.
pub fn find_ip_address(token: Token<'_>, state: &mut IpAddressState) {
    find_error_code(token, &mut state.error);
    if state.error.exit {
        return;
    }
    match token {
        Token::StartTag("NewExternalIPAddress") => state.in_ip_address = true,
        Token::String(s) if state.in_ip_address => {
            state.ip_address = s.to_owned();
            state.error.exit = true;
        }
        _ => {}
    }
}

/// Runs `find_control_url` over a whole device description.
pub fn parse_device_description(body: &str) -> ParseState {
    let mut s = ParseState::default();
    xml::parse(body, |t| find_control_url(t, &mut s));
    s
}

/// The error code of a SOAP reply, -1 when it has none.
pub fn parse_error_code(body: &str) -> i32 {
    let mut s = ErrorCodeState::new();
    xml::parse(body, |t| find_error_code(t, &mut s));
    s.error_code
}

/// The external address and the error code of a GetExternalIPAddress reply.
pub fn parse_external_address(body: &str) -> IpAddressState {
    let mut s = IpAddressState::new();
    xml::parse(body, |t| find_ip_address(t, &mut s));
    s
}

/// UPnP error codes (libtorrent `upnp_errors`).
pub const ERR_ACTION_FAILED: i32 = 501;
pub const ERR_PORT_MAPPING_CONFLICT: i32 = 718;
pub const ERR_ONLY_PERMANENT_LEASES_SUPPORTED: i32 = 725;
pub const ERR_EXTERNAL_PORT_MUST_BE_WILDCARD: i32 = 727;

const ERROR_CODES: [(i32, &str); 11] = [
    (0, "no error"),
    (402, "Invalid Arguments"),
    (501, "Action Failed"),
    (714, "The specified value does not exist in the array"),
    (715, "The source IP address cannot be wild-carded"),
    (716, "The external port cannot be wild-carded"),
    (718, "The port mapping entry specified conflicts with a mapping assigned previously to another client"),
    (724, "Internal and External port values must be the same"),
    (725, "The NAT implementation only supports permanent lease times on port mappings"),
    (726, "RemoteHost must be a wildcard and cannot be a specific IP address or DNS name"),
    (727, "ExternalPort must be a wildcard and cannot be a specific port "),
];

/// libtorrent `return_error`'s text: "UPnP mapping error <code>[: <message>]".
pub fn mapping_error(code: i32) -> String {
    let mut s = format!("UPnP mapping error {code}");
    if let Some((_, m)) = ERROR_CODES.iter().find(|(c, _)| *c == code) {
        s.push_str(": ");
        s.push_str(m);
    }
    s
}

pub fn protocol_name(p: Protocol) -> &'static str {
    match p {
        Protocol::Tcp => "TCP",
        Protocol::Udp => "UDP",
    }
}

/// libtorrent `upnp::post`: the HTTP request around a SOAP body.
pub fn post_request(
    path: &str,
    host: &str,
    port: u16,
    service_namespace: &str,
    action: &str,
    soap: &str,
) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\n\
Host: {host}:{port}\r\n\
Content-Type: text/xml; charset=\"utf-8\"\r\n\
Content-Length: {}\r\n\
Soapaction: \"{service_namespace}#{action}\"\r\n\r\n\
{soap}",
        soap.len()
    )
    .into_bytes()
}

fn envelope(action: &str, service_namespace: &str, inner: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body><u:{action} xmlns:u=\"{service_namespace}\">{inner}</u:{action}></s:Body></s:Envelope>"
    )
}

pub const ACTION_ADD: &str = "AddPortMapping";
pub const ACTION_DELETE: &str = "DeletePortMapping";
pub const ACTION_GET_IP: &str = "GetExternalIPAddress";

/// libtorrent `create_port_mapping`'s SOAP body.
pub fn add_port_mapping(
    service_namespace: &str,
    external_port: u16,
    protocol: Protocol,
    internal_port: u16,
    internal_client: &str,
    description: &str,
    lease_duration: u32,
) -> String {
    envelope(
        ACTION_ADD,
        service_namespace,
        &format!(
            "<NewRemoteHost></NewRemoteHost>\
<NewExternalPort>{external_port}</NewExternalPort>\
<NewProtocol>{}</NewProtocol>\
<NewInternalPort>{internal_port}</NewInternalPort>\
<NewInternalClient>{internal_client}</NewInternalClient>\
<NewEnabled>1</NewEnabled>\
<NewPortMappingDescription>{description}</NewPortMappingDescription>\
<NewLeaseDuration>{lease_duration}</NewLeaseDuration>",
            protocol_name(protocol)
        ),
    )
}

/// libtorrent `delete_port_mapping`'s SOAP body.
pub fn delete_port_mapping(service_namespace: &str, external_port: u16, protocol: Protocol) -> String {
    envelope(
        ACTION_DELETE,
        service_namespace,
        &format!(
            "<NewRemoteHost></NewRemoteHost>\
<NewExternalPort>{external_port}</NewExternalPort>\
<NewProtocol>{}</NewProtocol>",
            protocol_name(protocol)
        ),
    )
}

/// libtorrent `get_ip_address`'s SOAP body.
pub fn get_external_ip_address(service_namespace: &str) -> String {
    envelope(ACTION_GET_IP, service_namespace, "")
}

/// Whether a SOAP reply's content type is XML (libtorrent `on_upnp_map_response`); an absent
/// header passes.
pub fn is_xml_content_type(ct: Option<&str>) -> bool {
    match ct {
        None | Some("") => true,
        Some(ct) => ["text/xml", "text/soap+xml", "application/xml", "application/soap+xml"]
            .iter()
            .any(|t| ct.contains(t)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // test_xml.cpp's device descriptions
    const UPNP_XML: &str = r#"<root>
<specVersion>
<major>1</major>
<minor>0</minor>
</specVersion>
<URLBase>http://192.168.0.1:5678</URLBase>
<device>
<deviceType>
urn:schemas-upnp-org:device:InternetGatewayDevice:1
</deviceType>
<presentationURL>http://192.168.0.1:80</presentationURL>
<friendlyName>D-Link Router</friendlyName>
<manufacturer>D-Link</manufacturer>
<manufacturerURL>http://www.dlink.com</manufacturerURL>
<modelDescription>Internet Access Router</modelDescription>
<modelName>D-Link Router</modelName>
<UDN>uuid:upnp-InternetGatewayDevice-1_0-12345678900001</UDN>
<UPC>123456789001</UPC>
<serviceList>
<service>
<serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>
<serviceId>urn:upnp-org:serviceId:L3Forwarding1</serviceId>
<controlURL>/Layer3Forwarding</controlURL>
<eventSubURL>/Layer3Forwarding</eventSubURL>
<SCPDURL>/Layer3Forwarding.xml</SCPDURL>
</service>
</serviceList>
<deviceList>
<device>
<deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
<friendlyName>WANDevice</friendlyName>
<manufacturer>D-Link</manufacturer>
<manufacturerURL>http://www.dlink.com</manufacturerURL>
<modelDescription>Internet Access Router</modelDescription>
<modelName>D-Link Router</modelName>
<modelNumber>1</modelNumber>
<modelURL>http://support.dlink.com</modelURL>
<serialNumber>12345678900001</serialNumber>
<UDN>uuid:upnp-WANDevice-1_0-12345678900001</UDN>
<UPC>123456789001</UPC>
<serviceList>
<service>
<serviceType>
urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1
</serviceType>
<serviceId>urn:upnp-org:serviceId:WANCommonInterfaceConfig</serviceId>
<controlURL>/WANCommonInterfaceConfig</controlURL>
<eventSubURL>/WANCommonInterfaceConfig</eventSubURL>
<SCPDURL>/WANCommonInterfaceConfig.xml</SCPDURL>
</service>
</serviceList>
<deviceList>
<device>
<deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
<friendlyName>WAN Connection Device</friendlyName>
<manufacturer>D-Link</manufacturer>
<manufacturerURL>http://www.dlink.com</manufacturerURL>
<modelDescription>Internet Access Router</modelDescription>
<modelName>D-Link Router</modelName>
<modelNumber>1</modelNumber>
<modelURL>http://support.dlink.com</modelURL>
<serialNumber>12345678900001</serialNumber>
<UDN>uuid:upnp-WANConnectionDevice-1_0-12345678900001</UDN>
<UPC>123456789001</UPC>
<serviceList>
<service>
<serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
<serviceId>urn:upnp-org:serviceId:WANIPConnection</serviceId>
<controlURL>/WANIPConnection</controlURL>
<eventSubURL>/WANIPConnection</eventSubURL>
<SCPDURL>/WANIPConnection.xml</SCPDURL>
</service>
</serviceList>
</device>
</deviceList>
</device>
</deviceList>
</device>
</root>"#;

    const UPNP_XML2: &str = r#"<root>
<specVersion>
<major>1</major>
<minor>0</minor>
</specVersion>
<URLBase>http://192.168.1.1:49152</URLBase>
<device>
<deviceType>
urn:schemas-upnp-org:device:InternetGatewayDevice:1
</deviceType>
<friendlyName>LINKSYS WAG200G Gateway</friendlyName>
<manufacturer>LINKSYS</manufacturer>
<manufacturerURL>http://www.linksys.com</manufacturerURL>
<modelDescription>LINKSYS WAG200G Gateway</modelDescription>
<modelName>Wireless-G ADSL Home Gateway</modelName>
<modelNumber>WAG200G</modelNumber>
<modelURL>http://www.linksys.com</modelURL>
<serialNumber>123456789</serialNumber>
<UDN>uuid:8d401597-1dd2-11b2-a7d4-001ee5947cac</UDN>
<UPC>WAG200G</UPC>
<serviceList>
<service>
<serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>
<serviceId>urn:upnp-org:serviceId:L3Forwarding1</serviceId>
<controlURL>/upnp/control/L3Forwarding1</controlURL>
<eventSubURL>/upnp/event/L3Forwarding1</eventSubURL>
<SCPDURL>/l3frwd.xml</SCPDURL>
</service>
</serviceList>
<deviceList>
<device>
<deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
<friendlyName>WANDevice</friendlyName>
<manufacturer>LINKSYS</manufacturer>
<manufacturerURL>http://www.linksys.com/</manufacturerURL>
<modelDescription>Residential Gateway</modelDescription>
<modelName>Internet Connection Sharing</modelName>
<modelNumber>1</modelNumber>
<modelURL>http://www.linksys.com/</modelURL>
<serialNumber>0000001</serialNumber>
<UDN>uuid:8d401596-1dd2-11b2-a7d4-001ee5947cac</UDN>
<UPC>WAG200G</UPC>
<serviceList>
<service>
<serviceType>
urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1
</serviceType>
<serviceId>urn:upnp-org:serviceId:WANCommonIFC1</serviceId>
<controlURL>/upnp/control/WANCommonIFC1</controlURL>
<eventSubURL>/upnp/event/WANCommonIFC1</eventSubURL>
<SCPDURL>/cmnicfg.xml</SCPDURL>
</service>
</serviceList>
<deviceList>
<device>
<deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
<friendlyName>WANConnectionDevice</friendlyName>
<manufacturer>LINKSYS</manufacturer>
<manufacturerURL>http://www.linksys.com/</manufacturerURL>
<modelDescription>Residential Gateway</modelDescription>
<modelName>Internet Connection Sharing</modelName>
<modelNumber>1</modelNumber>
<modelURL>http://www.linksys.com/</modelURL>
<serialNumber>0000001</serialNumber>
<UDN>uuid:8d401597-1dd2-11b2-a7d3-001ee5947cac</UDN>
<UPC>WAG200G</UPC>
<serviceList>
<service>
<serviceType>
urn:schemas-upnp-org:service:WANEthernetLinkConfig:1
</serviceType>
<serviceId>urn:upnp-org:serviceId:WANEthLinkC1</serviceId>
<controlURL>/upnp/control/WANEthLinkC1</controlURL>
<eventSubURL>/upnp/event/WANEthLinkC1</eventSubURL>
<SCPDURL>/wanelcfg.xml</SCPDURL>
</service>
<service>
<serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</serviceType>
<serviceId>urn:upnp-org:serviceId:WANPPPConn1</serviceId>
<controlURL>/upnp/control/WANPPPConn1</controlURL>
<eventSubURL>/upnp/event/WANPPPConn1</eventSubURL>
<SCPDURL>/pppcfg.xml</SCPDURL>
</service>
</serviceList>
</device>
</deviceList>
</device>
<device>
<deviceType>urn:schemas-upnp-org:device:LANDevice:1</deviceType>
<friendlyName>LANDevice</friendlyName>
<manufacturer>LINKSYS</manufacturer>
<manufacturerURL>http://www.linksys.com/</manufacturerURL>
<modelDescription>Residential Gateway</modelDescription>
<modelName>Residential Gateway</modelName>
<modelNumber>1</modelNumber>
<modelURL>http://www.linksys.com/</modelURL>
<serialNumber>0000001</serialNumber>
<UDN>uuid:8d401596-1dd2-11b2-a7d3-001ee5947cac</UDN>
<UPC>WAG200G</UPC>
<serviceList>
<service>
<serviceType>
urn:schemas-upnp-org:service:LANHostConfigManagement:1
</serviceType>
<serviceId>urn:upnp-org:serviceId:LANHostCfg1</serviceId>
<controlURL>/upnp/control/LANHostCfg1</controlURL>
<eventSubURL>/upnp/event/LANHostCfg1</eventSubURL>
<SCPDURL>/lanhostc.xml</SCPDURL>
</service>
</serviceList>
</device>
</deviceList>
<presentationURL>http://192.168.1.1/index.htm</presentationURL>
</device>
</root>"#;

    const UPNP_XML3: &str = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
 s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body>
<s:Fault>
<faultcode>s:Client</faultcode>
<faultstring>UPnPError</faultstring>
<detail>
<UPnPErrorxmlns="urn:schemas-upnp-org:control-1-0">
<errorCode>402</errorCode>
<errorDescription>Invalid Args</errorDescription>
</UPnPError>
</detail>
</s:Fault>
</s:Body>
</s:Envelope>"#;

    const UPNP_XML4: &str = r#"<?xml version="1.0"?>
<s:Envelope
 xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
 s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body>
<u:GetExternalIPAddressResponse
 xmlns:u="urn:schemas-upnp-org:service:WANIPConnection:1">
<NewExternalIPAddress>123.10.20.30</NewExternalIPAddress>
</u:GetExternalIPAddressResponse>
</s:Body>
</s:Envelope>"#;

    const UPNP_XML5: &str = r#"<root>
<URLBase>http://192.168.1.1:49152</URLBase>
 <device>
  <deviceList>
   <device>
    <deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
     <deviceList>
      <device>
       <deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
       <friendlyName>WANConnectionDevice</friendlyName>
       <serviceList>
        <service>
         <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
         <serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
         <SCPDURL>/wipc_scpd.xml</SCPDURL>
         <controlURL>/wipc_cont</controlURL>
         <eventSubURL>/wipc_evnt</eventSubURL>
        </service>
        <service>
         <serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</serviceType>
         <serviceId>urn:upnp-org:serviceId:WANPPPConnection</serviceId>
         <SCPDURL>/wpppc_scpd.xml</SCPDURL>
         <controlURL>/wpppc_cont</controlURL>
         <eventSubURL>/wpppc_evnt</eventSubURL>
        </service>
      </serviceList>
     </device>
    </deviceList>
   </device>
  </deviceList>
  <presentationURL>http://192.168.1.1/index.htm</presentationURL>
 </device>
</root>"#;

    #[test]
    fn upnp_parser1() {
        let s = parse_device_description(UPNP_XML);
        assert_eq!(s.url_base, "http://192.168.0.1:5678");
        assert_eq!(s.control_url, "/WANIPConnection");
        assert_eq!(s.service_type, "urn:schemas-upnp-org:service:WANIPConnection:1");
        assert_eq!(s.model, "D-Link Router");
    }

    #[test]
    fn upnp_parser2() {
        let s = parse_device_description(UPNP_XML2);
        assert_eq!(s.url_base, "http://192.168.1.1:49152");
        assert_eq!(s.control_url, "/upnp/control/WANPPPConn1");
        assert_eq!(s.service_type, "urn:schemas-upnp-org:service:WANPPPConnection:1");
        assert_eq!(s.model, "Wireless-G ADSL Home Gateway");
    }

    #[test]
    fn upnp_parser3() {
        assert_eq!(parse_error_code(UPNP_XML3), 402);
        assert_eq!(parse_error_code("<a>1</a>"), -1);
        assert_eq!(parse_error_code("<errorCode> 718x</errorCode>"), 718);
        assert_eq!(parse_error_code("<errorCode>x</errorCode>"), 0);
        assert_eq!(parse_error_code("<errorCode>99999999999999999999</errorCode>"), i32::MAX);
    }

    #[test]
    fn upnp_parser4() {
        let s = parse_external_address(UPNP_XML4);
        assert_eq!(s.error.error_code, -1);
        assert_eq!(s.ip_address, "123.10.20.30");
        let s = parse_external_address(UPNP_XML3);
        assert_eq!(s.error.error_code, 402);
        assert_eq!(s.ip_address, "");
    }

    #[test]
    fn upnp_parser5() {
        let s = parse_device_description(UPNP_XML5);
        assert_eq!(s.url_base, "http://192.168.1.1:49152");
        assert_eq!(s.control_url, "/wipc_cont");
        assert_eq!(s.service_type, "urn:schemas-upnp-org:service:WANIPConnection:1");
    }

    #[test]
    fn ssdp_datagrams() {
        let resp = b"HTTP/1.1 200 OK\r\nST:upnp:rootdevice\r\nUSN:uuid:000f::upnp:rootdevice\r\n\
Location: http://192.168.1.1:5431/dyndev/uuid:000f\r\nServer: Custom/1.0 UPnP/1.0 Proc/Ver\r\nEXT:\r\n\
Cache-Control:max-age=180\r\nDATE: Fri, 02 Jan 1970 08:10:38 GMT\r\n\r\n";
        let (s, loc) = parse_ssdp(resp).unwrap();
        assert_eq!((s.status, s.method.as_str()), (200, ""));
        assert_eq!(loc, "http://192.168.1.1:5431/dyndev/uuid:000f");
        assert_eq!(s.header("ext"), Some(""));
        let notify = b"NOTIFY * HTTP/1.1\r\nHost:239.255.255.250:1900\r\nNT:urn:schemas-upnp-org:device:MediaServer:1\r\n\
NTS:ssdp:alive\r\nLocation:http://10.0.3.169:2869/upnphost/udhisapi.dll?content=uuid:c17f\r\n\r\n";
        let (s, loc) = parse_ssdp(notify).unwrap();
        assert_eq!((s.status, s.method.as_str()), (0, "notify"));
        assert_eq!(loc, "http://10.0.3.169:2869/upnphost/udhisapi.dll?content=uuid:c17f");
        // our own search comes back through multicast loopback
        assert_eq!(parse_ssdp(MSEARCH), Err(Invalid::Status));
        assert_eq!(parse_ssdp(b"HTTP/1.1 404 Not Found\r\n\r\n"), Err(Invalid::Status));
        assert_eq!(parse_ssdp(b"HTTP/1.1 200 OK\r\nLocation: x\r\n"), Err(Invalid::Incomplete));
        assert_eq!(parse_ssdp(b"HTTP/1.1 200 OK\r\nServer: x\r\n\r\n"), Err(Invalid::MissingLocation));
        assert_eq!(parse_ssdp(b"HTTP/1.1 200 OK\r\nLocation:\r\n\r\n"), Err(Invalid::MissingLocation));
        assert_eq!(parse_ssdp(b"\xff\xfe"), Err(Invalid::Malformed));
        assert_eq!(parse_ssdp(b"HTTP/1.1 OK\r\n\r\n"), Err(Invalid::Malformed));
        assert_eq!(parse_ssdp(b"GET /\r\n\r\n"), Err(Invalid::Malformed));
        assert_eq!(parse_ssdp(b""), Err(Invalid::Malformed));
        // a header line without a colon ends the header, as in libtorrent's parser
        let (s, _) =
            parse_ssdp(b"HTTP/1.1 200 OK\r\nLocation: http://a/\r\nodd line\r\nServer: x\r\n\r\n").unwrap();
        assert_eq!(s.header("server"), None);
    }

    #[test]
    fn soap_bodies_match_libtorrent() {
        let ns = "urn:schemas-upnp-org:service:WANIPConnection:1";
        let body = add_port_mapping(ns, 6881, Protocol::Tcp, 6881, "192.168.1.5", "PaTorrent/0.1.0", 3600);
        assert_eq!(
            body,
            "<?xml version=\"1.0\"?>\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:AddPortMapping \
xmlns:u=\"urn:schemas-upnp-org:service:WANIPConnection:1\"><NewRemoteHost></NewRemoteHost>\
<NewExternalPort>6881</NewExternalPort><NewProtocol>TCP</NewProtocol><NewInternalPort>6881</NewInternalPort>\
<NewInternalClient>192.168.1.5</NewInternalClient><NewEnabled>1</NewEnabled>\
<NewPortMappingDescription>PaTorrent/0.1.0</NewPortMappingDescription><NewLeaseDuration>3600</NewLeaseDuration>\
</u:AddPortMapping></s:Body></s:Envelope>"
        );
        let del = delete_port_mapping(ns, 6881, Protocol::Udp);
        assert!(del.contains("<u:DeletePortMapping xmlns:u=\"urn:schemas-upnp-org:service:WANIPConnection:1\">\
<NewRemoteHost></NewRemoteHost><NewExternalPort>6881</NewExternalPort><NewProtocol>UDP</NewProtocol></u:DeletePortMapping>"));
        let ip = get_external_ip_address(ns);
        assert!(ip.ends_with(
            "<s:Body><u:GetExternalIPAddress xmlns:u=\"urn:schemas-upnp-org:service:WANIPConnection:1\">\
</u:GetExternalIPAddress></s:Body></s:Envelope>"
        ));
        let req = post_request("/ctl", "192.168.1.1", 5000, ns, ACTION_ADD, &body);
        let req = String::from_utf8(req).unwrap();
        assert!(req.starts_with(
            "POST /ctl HTTP/1.1\r\nHost: 192.168.1.1:5000\r\nContent-Type: text/xml; charset=\"utf-8\"\r\n"
        ));
        assert!(req.contains(&format!(
            "Content-Length: {}\r\nSoapaction: \"{ns}#AddPortMapping\"\r\n\r\n",
            body.len()
        )));
        assert!(req.ends_with(&body));
    }

    #[test]
    fn error_messages() {
        assert_eq!(mapping_error(727), format!("UPnP mapping error 727: {}", ERROR_CODES[10].1));
        assert_eq!(mapping_error(600), "UPnP mapping error 600");
        assert!(is_xml_content_type(None));
        assert!(is_xml_content_type(Some("text/xml; charset=\"utf-8\"")));
        assert!(is_xml_content_type(Some("application/soap+xml")));
        assert!(!is_xml_content_type(Some("text/html")));
    }
}
