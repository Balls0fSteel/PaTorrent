// PaTorrent's own OS glue, not ported (libtorrent uses OpenSSL through Boost.Asio).

//! TLS sessions on SChannel (SSPI), Windows' TLS library, and server identities kept in CNG.
//!
//! A client's certificate validation is SChannel's automatic mode: the chain is built against
//! the Windows trust store and the end certificate must match the target name passed to
//! `InitializeSecurityContextW`, which also becomes the SNI host name. Revocation is not checked,
//! because SChannel would fetch CRLs synchronously on the engine thread. A pinned client turns
//! SChannel's validation off and compares the server certificate's fingerprint itself.
//!
//! A server's private key has to be a persisted CNG key: SChannel signs in LSASS, which opens
//! the key by name. [`Identity`] imports the PEM file's key under a name made of this process's
//! id and deletes it on drop; names whose process is gone are swept at the next import.

use std::ffi::c_void;
use std::io;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use windows_sys::Win32::Foundation::{
    CERT_E_CN_NO_MATCH, CERT_E_EXPIRED, CERT_E_UNTRUSTEDROOT, CloseHandle, ERROR_ACCESS_DENIED, GetLastError,
    SEC_E_ALGORITHM_MISMATCH, SEC_E_CERT_EXPIRED, SEC_E_DECRYPT_FAILURE, SEC_E_ILLEGAL_MESSAGE,
    SEC_E_INCOMPLETE_MESSAGE, SEC_E_INTERNAL_ERROR, SEC_E_INVALID_TOKEN, SEC_E_NO_CREDENTIALS, SEC_E_OK,
    SEC_E_UNTRUSTED_ROOT, SEC_E_WRONG_PRINCIPAL, SEC_I_CONTEXT_EXPIRED, SEC_I_CONTINUE_NEEDED,
    SEC_I_INCOMPLETE_CREDENTIALS, SEC_I_RENEGOTIATE, STILL_ACTIVE, SYSTEMTIME,
};
use windows_sys::Win32::Security::Authentication::Identity::{
    ASC_REQ_ALLOCATE_MEMORY, ASC_REQ_CONFIDENTIALITY, ASC_REQ_EXTENDED_ERROR, ASC_REQ_FLAGS,
    ASC_REQ_REPLAY_DETECT, ASC_REQ_SEQUENCE_DETECT, ASC_REQ_STREAM, AcceptSecurityContext,
    AcquireCredentialsHandleW, DecryptMessage, DeleteSecurityContext, EncryptMessage, FreeContextBuffer,
    FreeCredentialsHandle, ISC_REQ_ALLOCATE_MEMORY, ISC_REQ_CONFIDENTIALITY, ISC_REQ_FLAGS,
    ISC_REQ_REPLAY_DETECT, ISC_REQ_SEQUENCE_DETECT, ISC_REQ_STREAM, ISC_REQ_USE_SUPPLIED_CREDS,
    InitializeSecurityContextW, QueryContextAttributesW, SCH_CRED_AUTO_CRED_VALIDATION,
    SCH_CRED_MANUAL_CRED_VALIDATION, SCH_CRED_NO_DEFAULT_CREDS, SCH_CREDENTIALS, SCH_CREDENTIALS_VERSION,
    SCH_USE_STRONG_CRYPTO, SCHANNEL_CRED, SCHANNEL_CRED_VERSION, SECBUFFER_ALERT, SECBUFFER_DATA,
    SECBUFFER_EMPTY, SECBUFFER_EXTRA, SECBUFFER_STREAM_HEADER, SECBUFFER_STREAM_TRAILER, SECBUFFER_TOKEN,
    SECBUFFER_VERSION, SECPKG_ATTR_REMOTE_CERT_CONTEXT, SECPKG_ATTR_STREAM_SIZES, SECPKG_CRED_INBOUND,
    SECPKG_CRED_OUTBOUND, SP_PROT_PCT1_CLIENT, SP_PROT_PCT1_SERVER, SP_PROT_SSL2_CLIENT, SP_PROT_SSL2_SERVER,
    SP_PROT_SSL3_CLIENT, SP_PROT_SSL3_SERVER, SP_PROT_TLS1_0_CLIENT, SP_PROT_TLS1_0_SERVER,
    SP_PROT_TLS1_1_CLIENT, SP_PROT_TLS1_1_SERVER, SP_PROT_TLS1_2_SERVER, SecBuffer, SecBufferDesc,
    SecPkgContext_StreamSizes, TLS_PARAMETERS, UNISP_NAME_W,
};
use windows_sys::Win32::Security::Credentials::SecHandle;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_PUBLIC_KEY_BLOB, BCryptBuffer, BCryptBufferDesc, BCryptDestroyKey,
    BCryptExportKey, CERT_CONTEXT, CERT_CREATE_SELFSIGN_NO_KEY_INFO, CERT_EXTENSION, CERT_EXTENSIONS,
    CERT_KEY_PROV_INFO_PROP_ID, CERT_STORE_ADD_ALWAYS, CERT_STORE_PROV_MEMORY, CRYPT_ALGORITHM_IDENTIFIER,
    CRYPT_INTEGER_BLOB, CRYPT_KEY_PROV_INFO, CertAddEncodedCertificateToStore, CertCloseStore,
    CertCreateSelfSignCertificate, CertFreeCertificateContext, CertOpenStore,
    CertSetCertificateContextProperty, CryptImportPublicKeyInfoEx2, HCERTSTORE, MS_KEY_STORAGE_PROVIDER,
    NCRYPT_ALLOW_EXPORT_FLAG, NCRYPT_ALLOW_PLAINTEXT_EXPORT_FLAG, NCRYPT_EXPORT_POLICY_PROPERTY,
    NCRYPT_KEY_HANDLE, NCRYPT_OVERWRITE_KEY_FLAG, NCRYPT_PKCS8_PRIVATE_KEY_BLOB, NCRYPT_PROV_HANDLE,
    NCRYPT_SILENT_FLAG, NCRYPTBUFFER_PKCS_KEY_NAME, NCRYPTBUFFER_VERSION, NCryptCreatePersistedKey,
    NCryptDeleteKey, NCryptEnumKeys, NCryptExportKey, NCryptFinalizeKey, NCryptFreeBuffer, NCryptFreeObject,
    NCryptImportKey, NCryptKeyName, NCryptOpenKey, NCryptOpenStorageProvider, NCryptSetProperty,
    X509_ASN_ENCODING,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::pem::{self, der};
use super::{Check, Fingerprint, SubjectName};

const ISC_FLAGS: ISC_REQ_FLAGS = ISC_REQ_SEQUENCE_DETECT
    | ISC_REQ_REPLAY_DETECT
    | ISC_REQ_CONFIDENTIALITY
    | ISC_REQ_ALLOCATE_MEMORY
    | ISC_REQ_STREAM
    | ISC_REQ_USE_SUPPLIED_CREDS;

const ASC_FLAGS: ASC_REQ_FLAGS = ASC_REQ_SEQUENCE_DETECT
    | ASC_REQ_REPLAY_DETECT
    | ASC_REQ_CONFIDENTIALITY
    | ASC_REQ_ALLOCATE_MEMORY
    | ASC_REQ_STREAM
    | ASC_REQ_EXTENDED_ERROR;

/// Process-wide client credentials, one for verifying clients and one for pinned ones. SChannel
/// keys its session cache on them, so reconnects to the same tracker can resume sessions. They
/// live until the process exits.
fn client_credentials(pinned: bool) -> io::Result<SecHandle> {
    static VERIFYING: OnceLock<Result<SecHandle, i32>> = OnceLock::new();
    static PINNED: OnceLock<Result<SecHandle, i32>> = OnceLock::new();
    let (cell, validation) = if pinned {
        (&PINNED, SCH_CRED_MANUAL_CRED_VALIDATION)
    } else {
        (&VERIFYING, SCH_CRED_AUTO_CRED_VALIDATION)
    };
    let flags = validation | SCH_CRED_NO_DEFAULT_CREDS | SCH_USE_STRONG_CRYPTO;
    match *cell.get_or_init(|| acquire_credentials(SECPKG_CRED_OUTBOUND, flags, &mut [], false)) {
        Ok(h) => Ok(h),
        Err(code) => Err(error("TLS is not available", code)),
    }
}

/// Credentials for one direction; a server passes its certificate. `tls12` turns off what came
/// before TLS 1.2, whatever the system allows (a client keeps the system's choice, for old
/// trackers).
fn acquire_credentials(
    direction: u32,
    flags: u32,
    certs: &mut [*mut CERT_CONTEXT],
    tls12: bool,
) -> Result<SecHandle, i32> {
    let mut handle = SecHandle { dwLower: 0, dwUpper: 0 };
    let count = certs.len() as u32;
    let certs = if certs.is_empty() { ptr::null_mut() } else { certs.as_mut_ptr() };
    let mut old_protocols = TLS_PARAMETERS {
        grbitDisabledProtocols: SP_PROT_PCT1_CLIENT
            | SP_PROT_PCT1_SERVER
            | SP_PROT_SSL2_CLIENT
            | SP_PROT_SSL2_SERVER
            | SP_PROT_SSL3_CLIENT
            | SP_PROT_SSL3_SERVER
            | SP_PROT_TLS1_0_CLIENT
            | SP_PROT_TLS1_0_SERVER
            | SP_PROT_TLS1_1_CLIENT
            | SP_PROT_TLS1_1_SERVER,
        ..Default::default()
    };
    // SCH_CREDENTIALS (Windows 10 1809 and later) allows TLS 1.3; SCHANNEL_CRED stops at 1.2.
    let new = SCH_CREDENTIALS {
        dwVersion: SCH_CREDENTIALS_VERSION,
        cCreds: count,
        paCred: certs,
        dwFlags: flags,
        cTlsParameters: u32::from(tls12),
        pTlsParameters: if tls12 { &mut old_protocols } else { ptr::null_mut() },
        ..Default::default()
    };
    let old = SCHANNEL_CRED {
        dwVersion: SCHANNEL_CRED_VERSION,
        cCreds: count,
        paCred: certs,
        dwFlags: flags,
        grbitEnabledProtocols: if tls12 { SP_PROT_TLS1_2_SERVER } else { 0 },
        ..Default::default()
    };
    let mut status = 0;
    for auth in [(&raw const new).cast(), (&raw const old).cast()] {
        // SAFETY: `auth` points to a live, fully initialised SCH_CREDENTIALS or SCHANNEL_CRED whose
        // version field tells SChannel which one it is, and whose certificate array and TLS
        // parameters (if any) outlive the call; the package name is a static wide string and `handle` is a valid
        // out-pointer. Optional arguments are null.
        status = unsafe {
            AcquireCredentialsHandleW(
                ptr::null(),
                UNISP_NAME_W,
                direction,
                ptr::null(),
                auth,
                None,
                ptr::null(),
                &mut handle,
                ptr::null_mut(),
            )
        };
        if status == SEC_E_OK {
            return Ok(handle);
        }
    }
    Err(status)
}

fn error(what: &str, code: i32) -> io::Error {
    let why = match code {
        SEC_E_UNTRUSTED_ROOT | CERT_E_UNTRUSTEDROOT => "the certificate is not trusted",
        SEC_E_WRONG_PRINCIPAL | CERT_E_CN_NO_MATCH => "the certificate does not match the host name",
        SEC_E_CERT_EXPIRED | CERT_E_EXPIRED => "the certificate has expired or is not yet valid",
        SEC_E_ALGORITHM_MISMATCH => "no TLS version or cipher in common with the peer",
        SEC_E_ILLEGAL_MESSAGE => "the peer sent an invalid message or an alert",
        SEC_E_INVALID_TOKEN => "the peer does not speak TLS",
        SEC_E_DECRYPT_FAILURE => "corrupt TLS record",
        SEC_I_INCOMPLETE_CREDENTIALS => "the server requires a client certificate",
        SEC_E_NO_CREDENTIALS => "SChannel cannot use the certificate's private key",
        SEC_E_INTERNAL_ERROR => "SChannel internal error",
        _ => "SChannel error",
    };
    io::Error::other(format!("{what}: {why} (0x{:08X})", code as u32))
}

/// A failed CNG call (`SECURITY_STATUS`).
fn cng_error(what: &str, status: i32) -> io::Error {
    io::Error::other(format!("{what} (0x{:08X})", status as u32))
}

/// A failed CryptoAPI call (`GetLastError`).
fn last_error(what: &str) -> io::Error {
    io::Error::other(format!("{what}: {}", io::Error::last_os_error()))
}

fn buffer(kind: u32, data: *mut u8, len: usize) -> SecBuffer {
    SecBuffer { cbBuffer: len as u32, BufferType: kind, pvBuffer: data.cast() }
}

fn empty(kind: u32) -> SecBuffer {
    buffer(kind, ptr::null_mut(), 0)
}

fn desc(bufs: &mut [SecBuffer]) -> SecBufferDesc {
    SecBufferDesc { ulVersion: SECBUFFER_VERSION, cBuffers: bufs.len() as u32, pBuffers: bufs.as_mut_ptr() }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

enum Role {
    Client {
        /// The host name, NUL-terminated UTF-16.
        target: Vec<u16>,
        check: Check,
    },
    /// Keeps the server's credentials alive while the session uses them.
    Server { _identity: Arc<Identity> },
}

pub(crate) struct Session {
    cred: SecHandle,
    ctx: SecHandle,
    has_ctx: bool,
    role: Role,
    /// Ciphertext received but not consumed yet.
    input: Vec<u8>,
    out: Vec<u8>,
    handshaking: bool,
    sizes: SecPkgContext_StreamSizes,
    closed: bool,
    /// The server's certificate, as a client saw it.
    peer: Option<Fingerprint>,
}

impl Session {
    pub fn client(host: &str, check: Check) -> io::Result<Session> {
        if host.contains('\0') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid host name"));
        }
        let manual = !matches!(check, Check::System);
        let mut s = Session::with(client_credentials(manual)?, Role::Client { target: wide(host), check });
        s.handshake()?;
        Ok(s)
    }

    pub fn server(identity: &Arc<Identity>) -> io::Result<Session> {
        Ok(Session::with(identity.cred, Role::Server { _identity: identity.clone() }))
    }

    fn with(cred: SecHandle, role: Role) -> Session {
        Session {
            cred,
            ctx: SecHandle { dwLower: 0, dwUpper: 0 },
            has_ctx: false,
            role,
            input: Vec::new(),
            out: Vec::new(),
            handshaking: true,
            sizes: SecPkgContext_StreamSizes {
                cbHeader: 0,
                cbTrailer: 0,
                cbMaximumMessage: 0,
                cBuffers: 0,
                cbBlockSize: 0,
            },
            closed: false,
            peer: None,
        }
    }

    /// Runs InitializeSecurityContextW (client) or AcceptSecurityContext (server) over the
    /// buffered input until it needs more data. A client's first call (no context yet) produces
    /// the ClientHello from nothing. Also finishes post-handshake exchanges that DecryptMessage
    /// reports as SEC_I_RENEGOTIATE.
    fn handshake(&mut self) -> io::Result<()> {
        loop {
            let hello = !self.has_ctx && matches!(self.role, Role::Client { .. });
            if !hello && self.input.is_empty() {
                return Ok(());
            }
            // SChannel reads a plain-text request as the header of a long record and waits for
            // the rest; a client's first record is always a handshake record (22)
            if !hello && !self.has_ctx && self.input[0] != 22 {
                return Err(error("TLS handshake failed", SEC_E_INVALID_TOKEN));
            }
            let mut inbufs =
                [buffer(SECBUFFER_TOKEN, self.input.as_mut_ptr(), self.input.len()), empty(SECBUFFER_EMPTY)];
            let indesc = desc(&mut inbufs);
            let mut outbufs = [empty(SECBUFFER_TOKEN), empty(SECBUFFER_ALERT)];
            let mut outdesc = desc(&mut outbufs);
            let mut attrs = 0u32;
            let ctx: *mut SecHandle = &raw mut self.ctx;
            let old: *const SecHandle = if self.has_ctx { ctx } else { ptr::null() };
            let status = match &self.role {
                // SAFETY: the credentials handle is valid for the process lifetime; `old` is null on
                // the first call and our live context afterwards (SChannel updates it in place, as
                // its documentation allows); `target` is NUL-terminated; the input descriptor
                // points at `self.input`, which is not touched until the call returns; the output
                // buffers are allocated by SChannel (ISC_REQ_ALLOCATE_MEMORY) and freed below.
                Role::Client { target, .. } => unsafe {
                    InitializeSecurityContextW(
                        &self.cred,
                        old,
                        target.as_ptr(),
                        ISC_FLAGS,
                        0,
                        0,
                        if hello { ptr::null() } else { &indesc },
                        0,
                        ctx,
                        &mut outdesc,
                        &mut attrs,
                        ptr::null_mut(),
                    )
                },
                // SAFETY: as above, with the identity's credentials, which the session keeps alive
                // through its `Arc`; a server always passes the received input.
                Role::Server { .. } => unsafe {
                    AcceptSecurityContext(
                        &self.cred,
                        old,
                        &indesc,
                        ASC_FLAGS,
                        0,
                        ctx,
                        &mut outdesc,
                        &mut attrs,
                        ptr::null_mut(),
                    )
                },
            };
            if status >= 0 {
                self.has_ctx = true;
            }
            for b in &outbufs {
                if !b.pvBuffer.is_null() {
                    if b.BufferType == SECBUFFER_TOKEN && b.cbBuffer > 0 {
                        // SAFETY: SChannel allocated `cbBuffer` bytes at `pvBuffer` for this token.
                        let token = unsafe {
                            std::slice::from_raw_parts(b.pvBuffer as *const u8, b.cbBuffer as usize)
                        };
                        self.out.extend_from_slice(token);
                    }
                    // SAFETY: the buffer came from SChannel's allocator and is freed exactly once.
                    unsafe { FreeContextBuffer(b.pvBuffer) };
                }
            }
            match status {
                SEC_E_OK | SEC_I_CONTINUE_NEEDED => {
                    if !hello {
                        let extra = if inbufs[1].BufferType == SECBUFFER_EXTRA {
                            inbufs[1].cbBuffer as usize
                        } else {
                            0
                        };
                        let used = self.input.len() - extra.min(self.input.len());
                        self.input.drain(..used);
                    }
                    if status == SEC_E_OK {
                        self.handshaking = false;
                        self.query_sizes()?;
                        return self.check_peer();
                    }
                    if hello {
                        return Ok(());
                    }
                }
                SEC_E_INCOMPLETE_MESSAGE => return Ok(()),
                e => return Err(error("TLS handshake failed", e)),
            }
        }
    }

    fn query_sizes(&mut self) -> io::Result<()> {
        // SAFETY: the context is established, and SECPKG_ATTR_STREAM_SIZES fills exactly a
        // SecPkgContext_StreamSizes.
        let status = unsafe {
            QueryContextAttributesW(&self.ctx, SECPKG_ATTR_STREAM_SIZES, (&raw mut self.sizes).cast())
        };
        if status != SEC_E_OK {
            return Err(error("TLS setup failed", status));
        }
        Ok(())
    }

    /// A client notes the server's certificate and, when pinned, insists on it.
    fn check_peer(&mut self) -> io::Result<()> {
        let Role::Client { check, .. } = self.role else { return Ok(()) };
        let mut cert: *mut CERT_CONTEXT = ptr::null_mut();
        // SAFETY: the context is established; SECPKG_ATTR_REMOTE_CERT_CONTEXT stores a certificate
        // context pointer, which we own and free.
        let status = unsafe {
            QueryContextAttributesW(&self.ctx, SECPKG_ATTR_REMOTE_CERT_CONTEXT, (&raw mut cert).cast())
        };
        if status == SEC_E_OK && !cert.is_null() {
            // SAFETY: a certificate context holds `cbCertEncoded` bytes of DER at `pbCertEncoded`.
            let der =
                unsafe { std::slice::from_raw_parts((*cert).pbCertEncoded, (*cert).cbCertEncoded as usize) };
            self.peer = Some(Fingerprint::of(der));
            // SAFETY: the context came from QueryContextAttributesW and is freed once.
            unsafe { CertFreeCertificateContext(cert) };
        }
        match check {
            Check::Pinned(pin) if self.peer != Some(pin) => Err(io::Error::other(format!(
                "TLS handshake failed: the server's certificate is not the accepted one (it presented {})",
                self.peer.map_or_else(|| "none".to_owned(), |p| p.to_string())
            ))),
            _ => Ok(()),
        }
    }

    fn decrypt(&mut self, plain: &mut Vec<u8>) -> io::Result<()> {
        while !self.input.is_empty() && !self.handshaking && !self.closed {
            let mut bufs = [
                buffer(SECBUFFER_DATA, self.input.as_mut_ptr(), self.input.len()),
                empty(SECBUFFER_EMPTY),
                empty(SECBUFFER_EMPTY),
                empty(SECBUFFER_EMPTY),
            ];
            let d = desc(&mut bufs);
            // SAFETY: the context is established; the descriptor's data buffer is `self.input`,
            // which SChannel decrypts in place and which outlives the call.
            let status = unsafe { DecryptMessage(&self.ctx, &d, 0, ptr::null_mut()) };
            match status {
                SEC_E_INCOMPLETE_MESSAGE => return Ok(()),
                SEC_E_OK | SEC_I_RENEGOTIATE | SEC_I_CONTEXT_EXPIRED => {}
                e => return Err(error("TLS receive failed", e)),
            }
            let mut extra = 0;
            for b in &bufs {
                match b.BufferType {
                    SECBUFFER_DATA if b.cbBuffer > 0 => {
                        // SAFETY: SChannel points DATA at the decrypted bytes inside `self.input`.
                        let data = unsafe {
                            std::slice::from_raw_parts(b.pvBuffer as *const u8, b.cbBuffer as usize)
                        };
                        plain.extend_from_slice(data);
                    }
                    SECBUFFER_EXTRA => extra = b.cbBuffer as usize,
                    _ => {}
                }
            }
            let used = self.input.len() - extra.min(self.input.len());
            self.input.drain(..used);
            match status {
                SEC_I_CONTEXT_EXPIRED => self.closed = true,
                SEC_I_RENEGOTIATE => {
                    // TLS 1.3 post-handshake messages (session tickets, key updates): the
                    // remaining input goes back through the handshake call.
                    self.handshaking = true;
                    self.handshake()?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl super::TlsSession for Session {
    fn handshaking(&self) -> bool {
        self.handshaking
    }

    fn feed(&mut self, ciphertext: &[u8], plain: &mut Vec<u8>) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        self.input.extend_from_slice(ciphertext);
        if self.handshaking {
            self.handshake()?;
        }
        self.decrypt(plain)
    }

    fn encrypt(&mut self, mut plain: &[u8]) -> io::Result<()> {
        debug_assert!(!self.handshaking);
        let (h, t) = (self.sizes.cbHeader as usize, self.sizes.cbTrailer as usize);
        let max = (self.sizes.cbMaximumMessage as usize).max(1);
        while !plain.is_empty() {
            let n = plain.len().min(max);
            let start = self.out.len();
            self.out.resize(start + h + n + t, 0);
            self.out[start + h..start + h + n].copy_from_slice(&plain[..n]);
            let p = self.out[start..].as_mut_ptr();
            // SAFETY: header, data and trailer are adjacent, non-overlapping ranges of the
            // `h + n + t` bytes just reserved at `p`.
            let mut bufs = unsafe {
                [
                    buffer(SECBUFFER_STREAM_HEADER, p, h),
                    buffer(SECBUFFER_DATA, p.add(h), n),
                    buffer(SECBUFFER_STREAM_TRAILER, p.add(h + n), t),
                    empty(SECBUFFER_EMPTY),
                ]
            };
            let d = desc(&mut bufs);
            // SAFETY: the context is established and the buffers describe memory owned by
            // `self.out`, sized from the context's stream sizes; SChannel encrypts in place.
            let status = unsafe { EncryptMessage(&self.ctx, 0, &d, 0) };
            if status != SEC_E_OK {
                self.out.truncate(start);
                return Err(error("TLS send failed", status));
            }
            let len = (bufs[0].cbBuffer + bufs[1].cbBuffer + bufs[2].cbBuffer) as usize;
            self.out.truncate(start + len);
            plain = &plain[n..];
        }
        Ok(())
    }

    fn outgoing(&mut self) -> &mut Vec<u8> {
        &mut self.out
    }

    fn closed(&self) -> bool {
        self.closed
    }

    fn peer_fingerprint(&self) -> Option<Fingerprint> {
        self.peer
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.has_ctx {
            // SAFETY: the context was created by SChannel and is deleted once.
            unsafe { DeleteSecurityContext(&self.ctx) };
        }
    }
}

// --- Server identities ---------------------------------------------------------------------

/// Persisted keys are named this, then `<process id>.<counter>`.
const KEY_PREFIX: &str = "PaTorrent TLS ";

/// A server certificate in a private in-memory store (with its chain, so SChannel can send
/// it), tied to a persisted CNG key, and the inbound credentials made from them.
pub(crate) struct Identity {
    cred: SecHandle,
    has_cred: bool,
    store: HCERTSTORE,
    cert: *mut CERT_CONTEXT,
    /// The CNG key's name, NUL-terminated UTF-16.
    key_name: Vec<u16>,
}

// SAFETY: after construction the credentials handle is only passed to AcceptSecurityContext,
// which SChannel allows from any thread at once, and the store and certificate context are not
// touched until Drop, which runs once, after the last session (each holds an `Arc`).
unsafe impl Send for Identity {}
// SAFETY: as above.
unsafe impl Sync for Identity {}

impl Identity {
    pub fn new(certs: &[Vec<u8>], key: &pem::Block) -> io::Result<Identity> {
        let pkcs8 = pem::pkcs8(key)?;
        let provider = Provider::open()?;
        provider.sweep_stale_keys();
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!("{KEY_PREFIX}{}.{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        // from here on Drop cleans up whatever exists
        let mut id = Identity {
            cred: SecHandle { dwLower: 0, dwUpper: 0 },
            has_cred: false,
            store: ptr::null_mut(),
            cert: ptr::null_mut(),
            key_name: wide(&name),
        };
        let key = provider.import(&pkcs8, &id.key_name)?;
        // SAFETY: a memory store needs no parameters; it is closed in Drop.
        id.store = unsafe { CertOpenStore(CERT_STORE_PROV_MEMORY, 0, 0, 0, ptr::null()) };
        if id.store.is_null() {
            return Err(last_error("cannot create a certificate store"));
        }
        for (i, der) in certs.iter().enumerate() {
            let mut ctx: *mut CERT_CONTEXT = ptr::null_mut();
            // SAFETY: `der` is valid for its length; the store is live; for the first certificate
            // SChannel returns a context we own (freed in Drop).
            let ok = unsafe {
                CertAddEncodedCertificateToStore(
                    id.store,
                    X509_ASN_ENCODING,
                    der.as_ptr(),
                    der.len() as u32,
                    CERT_STORE_ADD_ALWAYS,
                    if i == 0 { &mut ctx } else { ptr::null_mut() },
                )
            };
            if ok == 0 {
                return Err(last_error(if i == 0 {
                    "the certificate cannot be read"
                } else {
                    "a certificate of the chain cannot be read"
                }));
            }
            if i == 0 {
                id.cert = ctx;
            }
        }
        if !key.matches(id.cert) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the private key does not belong to the certificate",
            ));
        }
        drop(key);
        let info = CRYPT_KEY_PROV_INFO {
            pwszContainerName: id.key_name.as_mut_ptr(),
            pwszProvName: MS_KEY_STORAGE_PROVIDER as *mut u16,
            // 0 and 0: a CNG key
            dwProvType: 0,
            dwFlags: 0,
            cProvParam: 0,
            rgProvParam: ptr::null_mut(),
            dwKeySpec: 0,
        };
        // SAFETY: the context is live; the property is copied, so `info` and the strings it
        // points to only need to live for the call.
        if unsafe {
            CertSetCertificateContextProperty(
                id.cert,
                CERT_KEY_PROV_INFO_PROP_ID,
                0,
                (&raw const info).cast(),
            )
        } == 0
        {
            return Err(last_error("cannot attach the private key to the certificate"));
        }
        id.cred = acquire_credentials(SECPKG_CRED_INBOUND, SCH_USE_STRONG_CRYPTO, &mut [id.cert], true)
            .map_err(|code| error("the certificate cannot be used", code))?;
        id.has_cred = true;
        Ok(id)
    }
}

impl Drop for Identity {
    fn drop(&mut self) {
        // SAFETY: each handle was created above and is released once.
        unsafe {
            if self.has_cred {
                FreeCredentialsHandle(&self.cred);
            }
            if !self.cert.is_null() {
                CertFreeCertificateContext(self.cert);
            }
            if !self.store.is_null() {
                CertCloseStore(self.store, 0);
            }
        }
        if let Ok(p) = Provider::open() {
            p.delete(&self.key_name);
        }
    }
}

/// The Microsoft Software Key Storage Provider.
struct Provider(NCRYPT_PROV_HANDLE);

impl Provider {
    fn open() -> io::Result<Provider> {
        let mut h = 0;
        // SAFETY: `h` is a valid out-pointer; the provider name is a static wide string.
        let status = unsafe { NCryptOpenStorageProvider(&mut h, MS_KEY_STORAGE_PROVIDER, 0) };
        if status != 0 {
            return Err(cng_error("cannot open the key storage provider", status));
        }
        Ok(Provider(h))
    }

    /// Imports a PKCS #8 key as the persisted key `name` (NUL-terminated), replacing one of
    /// that name.
    fn import(&self, pkcs8: &[u8], name: &[u16]) -> io::Result<Key> {
        let mut buf = BCryptBuffer {
            cbBuffer: (name.len() * 2) as u32,
            BufferType: NCRYPTBUFFER_PKCS_KEY_NAME,
            pvBuffer: name.as_ptr() as *mut c_void,
        };
        let params = BCryptBufferDesc { ulVersion: NCRYPTBUFFER_VERSION, cBuffers: 1, pBuffers: &mut buf };
        let mut key = 0;
        // SAFETY: the provider is open; `params` names the key with a NUL-terminated string that
        // outlives the call (CNG only reads it); `pkcs8` is valid for its length.
        let status = unsafe {
            NCryptImportKey(
                self.0,
                0,
                NCRYPT_PKCS8_PRIVATE_KEY_BLOB,
                &params,
                &mut key,
                pkcs8.as_ptr(),
                pkcs8.len() as u32,
                NCRYPT_OVERWRITE_KEY_FLAG | NCRYPT_SILENT_FLAG,
            )
        };
        if status != 0 {
            return Err(cng_error("the private key cannot be imported", status));
        }
        Ok(Key(key))
    }

    /// Deletes the persisted key `name` (NUL-terminated), if it exists.
    fn delete(&self, name: &[u16]) {
        let mut key = 0;
        // SAFETY: the provider is open, `name` is NUL-terminated, and NCryptDeleteKey frees the
        // handle on success; on failure we free it.
        unsafe {
            if NCryptOpenKey(self.0, &mut key, name.as_ptr(), 0, NCRYPT_SILENT_FLAG) == 0
                && NCryptDeleteKey(key, NCRYPT_SILENT_FLAG) != 0
            {
                NCryptFreeObject(key);
            }
        }
    }

    /// Deletes keys left by processes of ours that are gone (killed, or crashed).
    fn sweep_stale_keys(&self) {
        let own = std::process::id();
        let mut stale = Vec::new();
        let mut state: *mut c_void = ptr::null_mut();
        loop {
            let mut name: *mut NCryptKeyName = ptr::null_mut();
            // SAFETY: the provider is open; `name` and `state` are out-pointers CNG fills and we
            // free with NCryptFreeBuffer.
            if unsafe { NCryptEnumKeys(self.0, ptr::null(), &mut name, &mut state, NCRYPT_SILENT_FLAG) } != 0
            {
                break;
            }
            // SAFETY: on success `name` points to a key name whose `pszName` is NUL-terminated.
            let key = unsafe { from_wide((*name).pszName) };
            // SAFETY: allocated by NCryptEnumKeys, freed once.
            unsafe { NCryptFreeBuffer(name.cast()) };
            let pid =
                key.strip_prefix(KEY_PREFIX).and_then(|r| r.split('.').next()).and_then(|p| p.parse().ok());
            if pid.is_some_and(|pid| pid != own && !process_alive(pid)) {
                stale.push(wide(&key));
            }
        }
        if !state.is_null() {
            // SAFETY: the enumeration state came from NCryptEnumKeys and is freed once.
            unsafe { NCryptFreeBuffer(state) };
        }
        for name in stale {
            self.delete(&name);
        }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        // SAFETY: the handle came from NCryptOpenStorageProvider and is freed once.
        unsafe { NCryptFreeObject(self.0) };
    }
}

/// A CNG key handle.
struct Key(NCRYPT_KEY_HANDLE);

impl Key {
    /// Whether this is the private half of the certificate's public key. When either side cannot
    /// be exported for the comparison, the answer is yes and the handshake will tell.
    fn matches(&self, cert: *const CERT_CONTEXT) -> bool {
        let Some(ours) = export(|out, len, got| {
            // SAFETY: the key is live; CNG writes at most `len` bytes to `out` (null to ask the
            // size) and the size to `got`.
            unsafe {
                NCryptExportKey(
                    self.0,
                    0,
                    BCRYPT_PUBLIC_KEY_BLOB,
                    ptr::null(),
                    out,
                    len,
                    got,
                    NCRYPT_SILENT_FLAG,
                )
            }
        }) else {
            return true;
        };
        let mut public = ptr::null_mut();
        // SAFETY: `cert` is a live context whose CERT_INFO holds the subject public key info.
        if unsafe {
            CryptImportPublicKeyInfoEx2(
                X509_ASN_ENCODING,
                &(*(*cert).pCertInfo).SubjectPublicKeyInfo,
                0,
                ptr::null(),
                &mut public,
            )
        } == 0
        {
            return true;
        }
        let theirs = export(|out, len, got| {
            // SAFETY: as above, for the BCrypt key just imported.
            unsafe { BCryptExportKey(public, ptr::null_mut(), BCRYPT_PUBLIC_KEY_BLOB, out, len, got, 0) }
        });
        // SAFETY: imported above, destroyed once.
        unsafe { BCryptDestroyKey(public) };
        // The blobs start with a magic number that names the algorithm (ECDSA or ECDH for the
        // same curve point), then sizes and the key itself.
        theirs.is_none_or(|t| t.get(4..) == ours.get(4..))
    }
}

/// Runs a CNG export twice: for the size, then for the bytes.
fn export(f: impl Fn(*mut u8, u32, &mut u32) -> i32) -> Option<Vec<u8>> {
    let mut len = 0;
    if f(ptr::null_mut(), 0, &mut len) != 0 {
        return None;
    }
    let mut out = vec![0u8; len as usize];
    if f(out.as_mut_ptr(), len, &mut len) != 0 {
        return None;
    }
    out.truncate(len as usize);
    Some(out)
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from CNG and is freed once.
        unsafe { NCryptFreeObject(self.0) };
    }
}

/// A NUL-terminated wide string.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string.
unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    // SAFETY: the caller promises a terminator, so every read up to it is in bounds.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

/// Whether process `pid` is still running (or exists and is not ours to look at).
fn process_alive(pid: u32) -> bool {
    // SAFETY: plain calls; the handle is closed once.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0;
        let alive = GetExitCodeProcess(h, &mut code) == 0 || code == STILL_ACTIVE as u32;
        CloseHandle(h);
        alive
    }
}

// --- Self-signed certificates --------------------------------------------------------------

/// A blob over `data` (CryptoAPI only reads it).
fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

/// A Unix time as the UTC `SYSTEMTIME` CryptoAPI takes.
fn system_time(unix: i64) -> SYSTEMTIME {
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let (y, m, d) = pem::civil_from_days(days);
    SYSTEMTIME {
        wYear: y as u16,
        wMonth: m as u16,
        wDayOfWeek: 0,
        wDay: d as u16,
        wHour: (secs / 3600) as u16,
        wMinute: (secs / 60 % 60) as u16,
        wSecond: (secs % 60) as u16,
        wMilliseconds: 0,
    }
}

/// See [`super::generate_self_signed`]: a fresh exportable ECDSA P-256 key in memory, the
/// certificate from CertCreateSelfSignCertificate, both returned as PEM.
pub(crate) fn generate_self_signed(
    cn: &str,
    names: &[SubjectName],
    not_before: i64,
    not_after: i64,
) -> io::Result<(String, String)> {
    let provider = Provider::open()?;
    let mut handle = 0;
    // SAFETY: the provider is open; a null name makes an ephemeral key, which `Key` frees.
    let status = unsafe {
        NCryptCreatePersistedKey(provider.0, &mut handle, BCRYPT_ECDSA_P256_ALGORITHM, ptr::null(), 0, 0)
    };
    if status != 0 {
        return Err(cng_error("cannot create a key", status));
    }
    let key = Key(handle);
    let policy: u32 = NCRYPT_ALLOW_EXPORT_FLAG | NCRYPT_ALLOW_PLAINTEXT_EXPORT_FLAG;
    // SAFETY: the key is live and not finalized; the property value is a 4-byte DWORD.
    let status = unsafe {
        NCryptSetProperty(
            key.0,
            NCRYPT_EXPORT_POLICY_PROPERTY,
            (&raw const policy).cast(),
            4,
            NCRYPT_SILENT_FLAG,
        )
    };
    if status != 0 {
        return Err(cng_error("cannot make the key exportable", status));
    }
    // SAFETY: the key is live.
    let status = unsafe { NCryptFinalizeKey(key.0, NCRYPT_SILENT_FLAG) };
    if status != 0 {
        return Err(cng_error("cannot create a key", status));
    }

    // Name ::= SEQUENCE OF SET OF { commonName (2.5.4.3), UTF8String }
    let cn_attr = [&[0x06, 0x03, 0x55, 0x04, 0x03][..], &der(0x0c, cn.as_bytes())].concat();
    let subject = der(0x30, &der(0x31, &der(0x30, &cn_attr)));
    // subjectAltName: dNSName [2] and iPAddress [7]
    let alt: Vec<u8> = names
        .iter()
        .flat_map(|n| match n {
            SubjectName::Dns(d) => der(0x82, d.as_bytes()),
            SubjectName::Ip(std::net::IpAddr::V4(ip)) => der(0x87, &ip.octets()),
            SubjectName::Ip(std::net::IpAddr::V6(ip)) => der(0x87, &ip.octets()),
        })
        .collect();
    let san = der(0x30, &alt);
    // extKeyUsage: id-kp-serverAuth (1.3.6.1.5.5.7.3.1)
    let eku = der(0x30, &[0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01]);
    let mut ext = [
        CERT_EXTENSION { pszObjId: c"2.5.29.17".as_ptr() as *mut u8, fCritical: 0, Value: blob(&san) },
        CERT_EXTENSION { pszObjId: c"2.5.29.37".as_ptr() as *mut u8, fCritical: 0, Value: blob(&eku) },
    ];
    let extensions = CERT_EXTENSIONS { cExtension: ext.len() as u32, rgExtension: ext.as_mut_ptr() };
    let algorithm = CRYPT_ALGORITHM_IDENTIFIER {
        // ecdsa-with-SHA256
        pszObjId: c"1.2.840.10045.4.3.2".as_ptr() as *mut u8,
        Parameters: CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() },
    };
    let (start, end) = (system_time(not_before), system_time(not_after));
    let subject_blob = blob(&subject);
    // SAFETY: every pointer refers to a live local above; CryptoAPI reads them during the call
    // and returns a new context (or null), which we free.
    let cert = unsafe {
        CertCreateSelfSignCertificate(
            key.0,
            &subject_blob,
            CERT_CREATE_SELFSIGN_NO_KEY_INFO,
            ptr::null(),
            &algorithm,
            &start,
            &end,
            &extensions,
        )
    };
    if cert.is_null() {
        return Err(last_error("cannot create the certificate"));
    }
    // SAFETY: the context holds `cbCertEncoded` bytes of DER; it is freed once, after the copy.
    let cert_der = unsafe {
        let der = std::slice::from_raw_parts((*cert).pbCertEncoded, (*cert).cbCertEncoded as usize).to_vec();
        CertFreeCertificateContext(cert);
        der
    };
    let pkcs8 = export(|out, len, got| {
        // SAFETY: the key is live and exportable in plain text; CNG writes at most `len` bytes.
        unsafe {
            NCryptExportKey(
                key.0,
                0,
                NCRYPT_PKCS8_PRIVATE_KEY_BLOB,
                ptr::null(),
                out,
                len,
                got,
                NCRYPT_SILENT_FLAG,
            )
        }
    })
    .ok_or_else(|| io::Error::other("cannot export the new key"))?;
    Ok((pem::encode("CERTIFICATE", &cert_der), pem::encode("PRIVATE KEY", &pkcs8)))
}

/// For the tests of the certificates the Security.framework backend builds by hand: signs
/// `message` with a PKCS #8 P-256 key through CNG, as a DER `Ecdsa-Sig-Value`.
#[cfg(test)]
pub(crate) fn ecdsa_sign_for_test(pkcs8: &[u8], message: &[u8]) -> Vec<u8> {
    use windows_sys::Win32::Security::Cryptography::NCryptSignHash;
    let provider = Provider::open().unwrap();
    let mut handle = 0;
    // SAFETY: the provider is open; without parameters the key is ephemeral, freed by `Key`.
    let status = unsafe {
        NCryptImportKey(
            provider.0,
            0,
            NCRYPT_PKCS8_PRIVATE_KEY_BLOB,
            ptr::null(),
            &mut handle,
            pkcs8.as_ptr(),
            pkcs8.len() as u32,
            NCRYPT_SILENT_FLAG,
        )
    };
    assert_eq!(status, 0, "import");
    let key = Key(handle);
    let hash = crate::crypto::Sha256::digest(message).0;
    let (mut sig, mut len) = ([0u8; 64], 0);
    // SAFETY: the key is live; the hash and the signature buffer are valid for their lengths.
    let status = unsafe {
        NCryptSignHash(
            key.0,
            ptr::null(),
            hash.as_ptr(),
            32,
            sig.as_mut_ptr(),
            64,
            &mut len,
            NCRYPT_SILENT_FLAG,
        )
    };
    assert_eq!((status, len), (0, 64), "sign");
    // CNG gives r || s; DER integers are minimal and positive
    let int = |b: &[u8]| {
        let b = &b[b.iter().take_while(|&&x| x == 0).count().min(b.len() - 1)..];
        der(0x02, &[if b[0] & 0x80 != 0 { &[0][..] } else { &[] }, b].concat())
    };
    der(0x30, &[int(&sig[..32]), int(&sig[32..])].concat())
}

/// For the same tests: whether a certificate's signature verifies with its own public key.
#[cfg(test)]
pub(crate) fn self_signature_verifies(cert: &[u8]) -> bool {
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_VERIFY_CERT_SIGN_ISSUER_CERT, CRYPT_VERIFY_CERT_SIGN_SUBJECT_CERT,
        CertCreateCertificateContext, CryptVerifyCertificateSignatureEx,
    };
    // SAFETY: `cert` is valid for its length; the context is freed once.
    unsafe {
        let ctx = CertCreateCertificateContext(X509_ASN_ENCODING, cert.as_ptr(), cert.len() as u32);
        if ctx.is_null() {
            return false;
        }
        let ok = CryptVerifyCertificateSignatureEx(
            0,
            X509_ASN_ENCODING,
            CRYPT_VERIFY_CERT_SIGN_SUBJECT_CERT,
            ctx.cast(),
            CRYPT_VERIFY_CERT_SIGN_ISSUER_CERT,
            ctx.cast(),
            0,
            ptr::null_mut(),
        ) != 0;
        CertFreeCertificateContext(ctx);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_exists(name: &str) -> bool {
        let p = Provider::open().unwrap();
        let w = wide(name);
        let mut key = 0;
        // SAFETY: the provider is open and the name NUL-terminated; a found key is freed.
        unsafe {
            let found = NCryptOpenKey(p.0, &mut key, w.as_ptr(), 0, NCRYPT_SILENT_FLAG) == 0;
            if found {
                NCryptFreeObject(key);
            }
            found
        }
    }

    #[test]
    fn the_persisted_key_lives_as_long_as_the_identity() {
        let pem = super::super::generate_self_signed(&["localhost"], 1).unwrap();
        let id = super::super::Identity::from_pem(pem.cert_pem.as_bytes(), pem.key_pem.as_bytes()).unwrap();
        let w = &id.inner.key_name;
        let name = String::from_utf16_lossy(&w[..w.len() - 1]);
        assert!(name.starts_with(&format!("{KEY_PREFIX}{}.", std::process::id())), "{name}");
        assert!(key_exists(&name));
        let clone = id.clone();
        drop(id);
        assert!(key_exists(&name));
        drop(clone);
        assert!(!key_exists(&name));
    }

    #[test]
    fn keys_left_by_processes_that_are_gone_are_swept() {
        let p = Provider::open().unwrap();
        // no process has this id (Windows process ids are multiples of 4)
        let name = format!("{KEY_PREFIX}4294967293.0");
        let key = pem::private_key(include_bytes!("../../../tests/fixtures/tls/ec-pkcs8.pem")).unwrap();
        drop(p.import(&pem::pkcs8(&key).unwrap(), &wide(&name)).unwrap());
        assert!(key_exists(&name));
        p.sweep_stale_keys();
        assert!(!key_exists(&name));
    }
}
