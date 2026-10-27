//! Secrets kept in the preferences file (today only `lt.proxy_password`).
//!
//! On Windows a secret is sealed with DPAPI (`CryptProtectData`, current-user scope, with an
//! entropy constant) and written as `dpapi:<base64>`, so a copied `patorrent.ini` read under
//! another Windows account (or on another machine) gives an empty password instead of the
//! user's. Elsewhere there is no DPAPI and secrets stay plain, as µTorrent kept them in
//! `settings.dat` (libsecret is not used), in a file only its owner may read
//! (`patorrent_core::util::private_file`). Plain values written before are read on every
//! platform and sealed at the next save.

/// The marker in front of a sealed value.
pub const MARKER: &str = "dpapi:";

/// The value as written to the file: sealed on Windows (an empty one stays empty), plain
/// elsewhere, or when sealing fails.
pub fn seal(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    match imp::protect(plain.as_bytes()) {
        Some(blob) => format!("{MARKER}{}", patorrent_core::util::string::base64encode(&blob)),
        None => plain.to_owned(),
    }
}

/// The plain value of a stored one: a `dpapi:` value is unsealed (empty when it cannot be:
/// another account's, a corrupt blob, or not Windows); anything else is a plain value written
/// before and is taken as it is.
pub fn open(stored: &str) -> String {
    let Some(b64) = stored.strip_prefix(MARKER) else { return stored.to_owned() };
    patorrent_core::util::string::base64decode(b64.trim().as_bytes())
        .and_then(|blob| imp::unprotect(&blob))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

/// Whether a stored value is sealed.
pub fn is_sealed(stored: &str) -> bool {
    stored.starts_with(MARKER)
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    /// Mixed into every blob, so another program's plain `CryptUnprotectData` on the same
    /// account does not open it without knowing it.
    const ENTROPY: &[u8] = b"PaTorrent proxy password v1";

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    /// Copies a DPAPI output blob and frees it.
    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: DPAPI returned `cbData` bytes at `pbData`, allocated with LocalAlloc.
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        // SAFETY: the buffer is DPAPI's and is freed once, here.
        unsafe { LocalFree(out.pbData.cast()) };
        v
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: the input blobs point at live slices DPAPI only reads; `out` receives a
        // LocalAlloc'd buffer that `take` frees.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        (ok != 0).then(|| take(out))
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as in `protect`; no description is asked for.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        (ok != 0).then(|| take(out))
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn protect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }

    pub fn unprotect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_values_are_read_as_they_are() {
        assert_eq!(open("hunter2"), "hunter2");
        assert_eq!(open(""), "");
        assert_eq!(seal(""), "");
    }

    #[test]
    fn corrupt_or_foreign_blobs_open_empty() {
        assert_eq!(open("dpapi:"), "");
        assert_eq!(open("dpapi:not base64!"), "");
        // valid base64 that is no DPAPI blob of this account (what another account's is)
        assert_eq!(open("dpapi:AAECAwQFBgcICQoLDA0ODw=="), "");
    }

    #[cfg(windows)]
    #[test]
    fn sealed_on_windows() {
        let s = seal("pa ss=wörd");
        assert!(is_sealed(&s), "{s}");
        assert!(!s.contains("pa ss"));
        assert_eq!(open(&s), "pa ss=wörd");
        // a changed byte in the blob does not open
        let mut blob = patorrent_core::util::string::base64decode(&s.as_bytes()[MARKER.len()..]).unwrap();
        let n = blob.len();
        blob[n - 5] ^= 0x55;
        let bad = format!("{MARKER}{}", patorrent_core::util::string::base64encode(&blob));
        assert_eq!(open(&bad), "");
    }

    #[cfg(not(windows))]
    #[test]
    fn plain_elsewhere() {
        assert_eq!(seal("pw"), "pw");
        assert!(!is_sealed(&seal("pw")));
    }
}
