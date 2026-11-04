//! The search helper's releases: the newest Jackett release from the GitHub API, the asset for
//! this platform, and its download, checked against the SHA-256 the API lists for the asset
//! (its `digest`, `sha256:<hex>`). An asset without a digest is refused.

use std::io;

use patorrent_core::Sha256Hash;
use patorrent_core::crypto::Sha256;
use patorrent_core::fetch::{FetchOptions, FetchProxy, http_get, http_get_to};

use super::json;

/// The GitHub API's newest release of Jackett. `PATORRENT_HELPER_RELEASES` at run time names
/// another (a local fake, for a hand test).
pub const RELEASES_URL: &str = "https://api.github.com/repos/Jackett/Jackett/releases/latest";
/// The project's page, for the confirm box and the documentation.
pub const PROJECT_URL: &str = "https://github.com/Jackett/Jackett";

/// The largest archive accepted (the releases are 48-57 MB).
pub const MAX_ARCHIVE: usize = 256 << 20;

/// The release asset for an operating system and architecture (`std::env::consts` names),
/// `musl` for a musl build on Linux. Windows has one zip, a 32-bit x86 build that runs on
/// x86-64 and on ARM64 Windows (as `azure-pipelines.yml` builds it, `win-x86`); FreeBSD and
/// the rest have none.
pub fn asset_name(os: &str, arch: &str, musl: bool) -> Option<&'static str> {
    Some(match (os, arch, musl) {
        ("windows", "x86_64" | "x86" | "aarch64", _) => "Jackett.Binaries.Windows.zip",
        ("linux", "x86_64", false) => "Jackett.Binaries.LinuxAMDx64.tar.gz",
        ("linux", "aarch64", false) => "Jackett.Binaries.LinuxARM64.tar.gz",
        ("linux", "x86_64", true) => "Jackett.Binaries.LinuxMuslAMDx64.tar.gz",
        ("linux", "aarch64", true) => "Jackett.Binaries.LinuxMuslARM64.tar.gz",
        ("macos", "x86_64", _) => "Jackett.Binaries.macOS.tar.gz",
        ("macos", "aarch64", _) => "Jackett.Binaries.macOSARM64.tar.gz",
        _ => return None,
    })
}

/// This build's asset ([`asset_name`]).
pub fn platform_asset() -> Option<&'static str> {
    asset_name(std::env::consts::OS, std::env::consts::ARCH, cfg!(target_env = "musl"))
}

/// One downloadable file of a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub url: String,
    pub sha256: Sha256Hash,
}

/// A release and the asset picked for this platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The tag (`v0.24.3009`).
    pub version: String,
    pub asset: Asset,
}

/// The release in the GitHub API's answer `text`, with the asset named `wanted`.
pub fn parse_release(text: &str, wanted: &str) -> Result<Release, String> {
    let v = json::parse(text)?;
    let version = v.str("tag_name").filter(|t| !t.is_empty()).ok_or("the release has no tag")?.to_owned();
    let a = v
        .get("assets")
        .map(json::Json::as_array)
        .unwrap_or(&[])
        .iter()
        .find(|a| a.str("name") == Some(wanted))
        .ok_or_else(|| format!("{version} has no {wanted}"))?;
    let url = a.str("browser_download_url").filter(|u| u.starts_with("https://") || u.starts_with("http://"));
    let url = url.ok_or_else(|| format!("{wanted}: no download address"))?.to_owned();
    let sha256 = a
        .str("digest")
        .and_then(|d| d.strip_prefix("sha256:"))
        .and_then(Sha256Hash::from_hex)
        .ok_or_else(|| format!("{wanted}: the release lists no SHA-256 for it"))?;
    let size = a.get("size").and_then(json::Json::as_u64).unwrap_or(0);
    Ok(Release { version, asset: Asset { name: wanted.to_owned(), size, url, sha256 } })
}

fn releases_url() -> String {
    std::env::var("PATORRENT_HELPER_RELEASES").unwrap_or_else(|_| RELEASES_URL.to_owned())
}

/// Asks GitHub for the newest release and picks this platform's asset (blocking). Through the
/// proxy when one is set (github.com is a public address, D41's rule).
pub fn latest(proxy: Option<FetchProxy>) -> Result<Release, String> {
    latest_from(&releases_url(), proxy)
}

/// [`latest`] from `url`.
pub fn latest_from(url: &str, proxy: Option<FetchProxy>) -> Result<Release, String> {
    let wanted = platform_asset().ok_or("Jackett has no build for this system")?;
    let opts = FetchOptions {
        proxy,
        max_size: 4 << 20,
        no_downgrade: true,
        headers: vec![
            ("Accept".to_owned(), "application/vnd.github+json".to_owned()),
            ("X-GitHub-Api-Version".to_owned(), "2022-11-28".to_owned()),
        ],
        ..FetchOptions::default()
    };
    let resp = http_get(url, &opts).map_err(|e| e.to_string())?;
    if resp.redirect().is_some() {
        return Err(format!("HTTP {}", resp.status));
    }
    parse_release(&String::from_utf8_lossy(&resp.body), wanted)
}

/// Downloads `release`'s asset into memory and checks it against its SHA-256 (blocking).
/// `progress` hears the running total and the size when known, and stops the download by
/// answering `false`.
pub fn download(
    release: &Release,
    proxy: Option<FetchProxy>,
    progress: &mut dyn FnMut(u64, Option<u64>) -> bool,
) -> Result<Vec<u8>, String> {
    let opts = FetchOptions {
        proxy,
        max_size: MAX_ARCHIVE,
        no_downgrade: true,
        timeout: std::time::Duration::from_secs(60),
        ..FetchOptions::default()
    };
    let mut data = Vec::with_capacity(release.asset.size.min(MAX_ARCHIVE as u64) as usize);
    let mut hash = Sha256::new();
    let resp = http_get_to(&release.asset.url, &opts, &mut |total, piece| {
        data.extend_from_slice(piece);
        hash.update(piece);
        if progress(data.len() as u64, total.or((release.asset.size > 0).then_some(release.asset.size))) {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
        }
    })
    .map_err(|e| format!("{}: {e}", release.asset.name))?;
    if resp.redirect().is_some() {
        return Err(format!("{}: HTTP {}", release.asset.name, resp.status));
    }
    if hash.finalize() != release.asset.sha256 {
        return Err(format!(
            "{}: the downloaded file's SHA-256 does not match the release's",
            release.asset.name
        ));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_for_each_platform() {
        assert_eq!(asset_name("windows", "x86_64", false), Some("Jackett.Binaries.Windows.zip"));
        assert_eq!(asset_name("windows", "aarch64", false), Some("Jackett.Binaries.Windows.zip"));
        assert_eq!(asset_name("linux", "x86_64", false), Some("Jackett.Binaries.LinuxAMDx64.tar.gz"));
        assert_eq!(asset_name("linux", "aarch64", false), Some("Jackett.Binaries.LinuxARM64.tar.gz"));
        assert_eq!(asset_name("linux", "x86_64", true), Some("Jackett.Binaries.LinuxMuslAMDx64.tar.gz"));
        assert_eq!(asset_name("macos", "x86_64", false), Some("Jackett.Binaries.macOS.tar.gz"));
        assert_eq!(asset_name("macos", "aarch64", false), Some("Jackett.Binaries.macOSARM64.tar.gz"));
        assert_eq!(asset_name("freebsd", "x86_64", false), None);
        assert_eq!(asset_name("linux", "riscv64", false), None);
        assert!(platform_asset().is_some() || cfg!(target_os = "freebsd"));
    }

    /// The shape of `releases/latest` (trimmed from the v0.24.3009 answer).
    pub const LATEST: &str = r#"{"tag_name":"v0.24.3009","name":"v0.24.3009","assets":[
        {"name":"Jackett.Binaries.LinuxAMDx64.tar.gz","size":50331648,
         "browser_download_url":"https://github.com/Jackett/Jackett/releases/download/v0.24.3009/Jackett.Binaries.LinuxAMDx64.tar.gz",
         "digest":"sha256:5f0c1a7e8d0b4f6c2a3e1d9b8c7a6f5e4d3c2b1a0f9e8d7c6b5a4f3e2d1c0b9a"},
        {"name":"Jackett.Binaries.Windows.zip","size":59768832,
         "browser_download_url":"https://github.com/Jackett/Jackett/releases/download/v0.24.3009/Jackett.Binaries.Windows.zip",
         "digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"},
        {"name":"Jackett.Binaries.macOS.tar.gz","size":1,"browser_download_url":"https://x/y"}]}"#;

    #[test]
    fn releases_parse() {
        let r = parse_release(LATEST, "Jackett.Binaries.Windows.zip").unwrap();
        assert_eq!(r.version, "v0.24.3009");
        assert_eq!(r.asset.size, 59768832);
        assert!(r.asset.url.ends_with("/v0.24.3009/Jackett.Binaries.Windows.zip"));
        assert_eq!(r.asset.sha256.to_hex(), "0123456789abcdef".repeat(4));
        let e = parse_release(LATEST, "Jackett.Binaries.macOS.tar.gz").unwrap_err();
        assert!(e.contains("SHA-256"), "no digest, no download: {e}");
        assert!(parse_release(LATEST, "Jackett.Binaries.Other.zip").unwrap_err().contains("has no"));
        assert!(parse_release("{}", "x").is_err());
        assert!(parse_release("<html>", "x").is_err());
    }
}
