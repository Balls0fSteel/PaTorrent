//! Stand-in until the platform detection lands (D48 part 3): every drive is Unknown.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DriveKind {
    Ssd,
    Hdd,
    Network,
    Removable,
    Unknown,
}

impl DriveKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            DriveKind::Ssd => "ssd",
            DriveKind::Hdd => "hdd",
            DriveKind::Network => "network",
            DriveKind::Removable => "removable",
            DriveKind::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<DriveKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ssd" => Some(DriveKind::Ssd),
            "hdd" => Some(DriveKind::Hdd),
            "network" => Some(DriveKind::Network),
            "removable" => Some(DriveKind::Removable),
            "unknown" => Some(DriveKind::Unknown),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DriveId {
    pub key: String,
    pub kind: DriveKind,
    pub describe: String,
}

pub(crate) fn detect(path: &Path) -> DriveId {
    let p = path.to_string_lossy().into_owned();
    DriveId { key: p.clone(), kind: DriveKind::Unknown, describe: format!("{p} (unknown)") }
}

pub(crate) fn overridden(overrides: &str, path: &Path) -> Option<DriveKind> {
    let path = path.to_string_lossy();
    overrides
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(p, _)| path.starts_with(p.trim()))
        .max_by_key(|(p, _)| p.trim().len())
        .and_then(|(_, k)| DriveKind::parse(k))
}
