//! UPnP input: SSDP datagrams, the XML scanner, and the device description and SOAP reply
//! scanners built on it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::{
    XmlToken, parse_device_description, parse_error_code, parse_external_address, parse_ssdp, parse_xml,
};

fuzz_target!(|data: &[u8]| {
    let _ = parse_ssdp(data);
    let text = String::from_utf8_lossy(data);
    let mut tokens = 0usize;
    parse_xml(&text, |t| {
        tokens += 1;
        if let XmlToken::Attribute(k, _) = t {
            assert!(!k.is_empty() || k.is_empty());
        }
    });
    let s = parse_device_description(&text);
    let _ = (s.control_url.len(), s.service_type.len(), s.model.len(), s.url_base.len());
    let _ = parse_error_code(&text);
    let _ = parse_external_address(&text);
});
