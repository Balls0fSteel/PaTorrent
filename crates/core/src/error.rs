//! Engine error type. Grows into the port of libtorrent's `error_code.cpp` categories
//! (`errors::`, bdecode, http, gzip, upnp, socks, system) as those subsystems land.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// libtorrent `errors::duplicate_torrent`.
    DuplicateTorrent,
    /// libtorrent `errors::invalid_torrent_handle`: the torrent is gone.
    InvalidTorrentHandle,
    /// The `.torrent` file could not be parsed.
    InvalidTorrentFile(String),
    /// The magnet link could not be parsed.
    InvalidMagnet(String),
    /// Resume data could not be parsed or does not match the torrent.
    InvalidResumeData(String),
    /// An OS-level I/O failure.
    Io { kind: std::io::ErrorKind, message: String },
    /// The session has shut down.
    SessionClosed,
    /// Not implemented yet in this phase of the port.
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::DuplicateTorrent => f.write_str("the torrent is already in the list"),
            Error::InvalidTorrentHandle => f.write_str("invalid torrent handle"),
            Error::InvalidTorrentFile(m) => write!(f, "invalid torrent file: {m}"),
            Error::InvalidMagnet(m) => write!(f, "invalid magnet link: {m}"),
            Error::InvalidResumeData(m) => write!(f, "invalid resume data: {m}"),
            Error::Io { message, .. } => f.write_str(message),
            Error::SessionClosed => f.write_str("the session has been shut down"),
            Error::Unsupported(what) => write!(f, "not supported yet: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io { kind: e.kind(), message: e.to_string() }
    }
}
