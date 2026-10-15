// Error codes and messages from libtorrent-rasterbar 2.0.11: include/libtorrent/error_code.hpp,
// src/error_code.cpp. Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause.

use std::fmt;

use crate::bencode::BdecodeErrorKind;

use super::file_storage::FileStorageError;

/// Why metadata (a `.torrent` file or magnet link) was rejected. Variant names follow libtorrent's
/// `errors::` enum; [`MetainfoError::message`] returns libtorrent's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetainfoError {
    Bdecode(BdecodeErrorKind),
    TorrentIsNoDict,
    TorrentMissingInfo,
    TorrentInfoNoDict,
    TorrentMissingPieceLength,
    TorrentMissingName,
    TorrentInvalidName,
    TorrentInvalidLength,
    TorrentFileParseFailed,
    TorrentMissingPieces,
    TorrentInvalidHashes,
    TooManyPiecesInTorrent,
    InvalidBencoding,
    NoFilesInTorrent,
    MetadataTooLarge,
    TorrentUnknownVersion,
    TorrentMissingFileTree,
    TorrentMissingMetaVersion,
    TorrentInconsistentFiles,
    TorrentMissingPieceLayer,
    TorrentInvalidPieceLayer,
    TorrentMissingPiecesRoot,
    TorrentInvalidPadFile,
    FileTooLarge,
    FilenameTooLong,
    InvalidEscapedString,
    MissingInfoHashInUri,
    UnsupportedUrlProtocol,
    InvalidInfoHash,
}

impl MetainfoError {
    pub fn message(self) -> &'static str {
        use MetainfoError::*;
        match self {
            Bdecode(k) => k.message(),
            TorrentIsNoDict => "torrent file is not a dictionary",
            TorrentMissingInfo => "missing or invalid 'info' section in torrent file",
            TorrentInfoNoDict => "'info' entry is not a dictionary",
            TorrentMissingPieceLength => "invalid or missing 'piece length' entry in torrent file",
            TorrentMissingName => "missing name in torrent file",
            TorrentInvalidName => "invalid 'name' of torrent (possible exploit attempt)",
            TorrentInvalidLength => "invalid length of torrent",
            TorrentFileParseFailed => "failed to parse files from torrent file",
            TorrentMissingPieces => "invalid or missing 'pieces' entry in torrent file",
            TorrentInvalidHashes => "incorrect number of piece hashes in torrent file",
            TooManyPiecesInTorrent => "too many pieces in torrent",
            InvalidBencoding => "invalid bencoding",
            NoFilesInTorrent => "no files in torrent",
            MetadataTooLarge => "metadata too large",
            TorrentUnknownVersion => "the torrent file has an unknown meta version",
            TorrentMissingFileTree => "the v2 torrent file has no file tree",
            TorrentMissingMetaVersion => "the torrent contains v2 keys but does not specify meta version 2",
            TorrentInconsistentFiles => "the v1 and v2 file metadata does not match",
            TorrentMissingPieceLayer => "one or more files are missing piece layer hashes",
            TorrentInvalidPieceLayer => "a piece layer is invalid",
            TorrentMissingPiecesRoot => "a v2 file entry has no root hash",
            TorrentInvalidPadFile => "a file in the v2 metadata has the pad attribute set",
            FileTooLarge => "File too large",
            FilenameTooLong => "Filename too long",
            InvalidEscapedString => "invalid escaped string",
            MissingInfoHashInUri => "missing info-hash from URI",
            UnsupportedUrlProtocol => "unsupported URL protocol",
            InvalidInfoHash => "invalid info-hash",
        }
    }
}

impl fmt::Display for MetainfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for MetainfoError {}

impl From<FileStorageError> for MetainfoError {
    fn from(e: FileStorageError) -> MetainfoError {
        match e {
            FileStorageError::FileTooLarge => MetainfoError::FileTooLarge,
            FileStorageError::FilenameTooLong => MetainfoError::FilenameTooLong,
            FileStorageError::InvalidLength => MetainfoError::TorrentInvalidLength,
            FileStorageError::MissingPiecesRoot => MetainfoError::TorrentMissingPiecesRoot,
            FileStorageError::InconsistentFiles => MetainfoError::TorrentInconsistentFiles,
        }
    }
}
