//! Sans-IO protocol codecs: peer wire, HTTP, trackers, ut_metadata, ut_pex, LSD, NAT-PMP / PCP,
//! UPnP and the XML scanner it needs, and the HTTP side of web seeds.

pub(crate) mod http;
pub(crate) mod http_parser;
pub(crate) mod lsd;
pub(crate) mod natpmp;
pub(crate) mod pe_handshake;
pub(crate) mod peer_wire;
pub(crate) mod tracker;
pub(crate) mod upnp;
pub(crate) mod ut_metadata;
pub(crate) mod ut_pex;
pub(crate) mod web_peer;
pub(crate) mod xml;
