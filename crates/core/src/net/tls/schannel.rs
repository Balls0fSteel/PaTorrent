// LibreTorrent's own OS glue, not ported (libtorrent uses OpenSSL through Boost.Asio).

//! TLS client sessions on SChannel (SSPI), Windows' TLS library.
//!
//! Certificate validation is SChannel's automatic mode: the chain is built against the Windows
//! trust store and the end certificate must match the target name passed to
//! `InitializeSecurityContextW`, which also becomes the SNI host name. Revocation is not checked,
//! because SChannel would fetch CRLs synchronously on the engine thread.

use std::io;
use std::ptr;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{
    CERT_E_CN_NO_MATCH, CERT_E_EXPIRED, CERT_E_UNTRUSTEDROOT, SEC_E_ALGORITHM_MISMATCH, SEC_E_CERT_EXPIRED,
    SEC_E_DECRYPT_FAILURE, SEC_E_ILLEGAL_MESSAGE, SEC_E_INCOMPLETE_MESSAGE, SEC_E_INVALID_TOKEN, SEC_E_OK,
    SEC_E_UNTRUSTED_ROOT, SEC_E_WRONG_PRINCIPAL, SEC_I_CONTEXT_EXPIRED, SEC_I_CONTINUE_NEEDED,
    SEC_I_INCOMPLETE_CREDENTIALS, SEC_I_RENEGOTIATE,
};
use windows_sys::Win32::Security::Authentication::Identity::{
    AcquireCredentialsHandleW, DecryptMessage, DeleteSecurityContext, EncryptMessage, FreeContextBuffer,
    ISC_REQ_ALLOCATE_MEMORY, ISC_REQ_CONFIDENTIALITY, ISC_REQ_FLAGS, ISC_REQ_REPLAY_DETECT,
    ISC_REQ_SEQUENCE_DETECT, ISC_REQ_STREAM, ISC_REQ_USE_SUPPLIED_CREDS, InitializeSecurityContextW,
    QueryContextAttributesW, SCH_CRED_AUTO_CRED_VALIDATION, SCH_CRED_NO_DEFAULT_CREDS, SCH_CREDENTIALS,
    SCH_CREDENTIALS_VERSION, SCH_USE_STRONG_CRYPTO, SCHANNEL_CRED, SCHANNEL_CRED_VERSION, SECBUFFER_ALERT,
    SECBUFFER_DATA, SECBUFFER_EMPTY, SECBUFFER_EXTRA, SECBUFFER_STREAM_HEADER, SECBUFFER_STREAM_TRAILER,
    SECBUFFER_TOKEN, SECBUFFER_VERSION, SECPKG_ATTR_STREAM_SIZES, SECPKG_CRED_OUTBOUND, SecBuffer,
    SecBufferDesc, SecPkgContext_StreamSizes, UNISP_NAME_W,
};
use windows_sys::Win32::Security::Credentials::SecHandle;

const ISC_FLAGS: ISC_REQ_FLAGS = ISC_REQ_SEQUENCE_DETECT
    | ISC_REQ_REPLAY_DETECT
    | ISC_REQ_CONFIDENTIALITY
    | ISC_REQ_ALLOCATE_MEMORY
    | ISC_REQ_STREAM
    | ISC_REQ_USE_SUPPLIED_CREDS;

/// One process-wide credentials handle. SChannel keys its session cache on it, so reconnects to
/// the same tracker can resume sessions. It lives until the process exits.
fn credentials() -> io::Result<SecHandle> {
    static CRED: OnceLock<Result<SecHandle, i32>> = OnceLock::new();
    match *CRED.get_or_init(acquire_credentials) {
        Ok(h) => Ok(h),
        Err(code) => Err(error("TLS is not available", code)),
    }
}

fn acquire_credentials() -> Result<SecHandle, i32> {
    let flags = SCH_CRED_AUTO_CRED_VALIDATION | SCH_CRED_NO_DEFAULT_CREDS | SCH_USE_STRONG_CRYPTO;
    let mut handle = SecHandle { dwLower: 0, dwUpper: 0 };
    // SCH_CREDENTIALS (Windows 10 1809 and later) allows TLS 1.3; SCHANNEL_CRED stops at 1.2.
    let new = SCH_CREDENTIALS { dwVersion: SCH_CREDENTIALS_VERSION, dwFlags: flags, ..Default::default() };
    let old = SCHANNEL_CRED { dwVersion: SCHANNEL_CRED_VERSION, dwFlags: flags, ..Default::default() };
    let mut status = 0;
    for auth in [(&raw const new).cast(), (&raw const old).cast()] {
        // SAFETY: `auth` points to a live, fully initialised SCH_CREDENTIALS or SCHANNEL_CRED whose
        // version field tells SChannel which one it is; the package name is a static wide string
        // and `handle` is a valid out-pointer. Optional arguments are null.
        status = unsafe {
            AcquireCredentialsHandleW(
                ptr::null(),
                UNISP_NAME_W,
                SECPKG_CRED_OUTBOUND,
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
        SEC_E_ALGORITHM_MISMATCH => "no TLS version or cipher in common with the server",
        SEC_E_ILLEGAL_MESSAGE => "the server sent an invalid message or an alert",
        SEC_E_INVALID_TOKEN => "the server does not speak TLS",
        SEC_E_DECRYPT_FAILURE => "corrupt TLS record",
        SEC_I_INCOMPLETE_CREDENTIALS => "the server requires a client certificate",
        _ => "SChannel error",
    };
    io::Error::other(format!("{what}: {why} (0x{:08X})", code as u32))
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

pub(crate) struct Session {
    cred: SecHandle,
    ctx: SecHandle,
    has_ctx: bool,
    /// The host name, NUL-terminated UTF-16.
    target: Vec<u16>,
    /// Ciphertext received but not consumed yet.
    input: Vec<u8>,
    out: Vec<u8>,
    handshaking: bool,
    sizes: SecPkgContext_StreamSizes,
    closed: bool,
}

impl Session {
    pub fn new(host: &str) -> io::Result<Session> {
        if host.contains('\0') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid host name"));
        }
        let mut s = Session {
            cred: credentials()?,
            ctx: SecHandle { dwLower: 0, dwUpper: 0 },
            has_ctx: false,
            target: host.encode_utf16().chain([0]).collect(),
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
        };
        s.handshake()?;
        Ok(s)
    }

    /// Runs InitializeSecurityContextW over the buffered input until it needs more data. The
    /// first call (no context yet) produces the ClientHello. Also finishes post-handshake
    /// exchanges that DecryptMessage reports as SEC_I_RENEGOTIATE.
    fn handshake(&mut self) -> io::Result<()> {
        loop {
            let first = !self.has_ctx;
            if !first && self.input.is_empty() {
                return Ok(());
            }
            let mut inbufs =
                [buffer(SECBUFFER_TOKEN, self.input.as_mut_ptr(), self.input.len()), empty(SECBUFFER_EMPTY)];
            let indesc = desc(&mut inbufs);
            let mut outbufs = [empty(SECBUFFER_TOKEN), empty(SECBUFFER_ALERT)];
            let mut outdesc = desc(&mut outbufs);
            let mut attrs = 0u32;
            let ctx: *mut SecHandle = &raw mut self.ctx;
            // SAFETY: the credentials handle is valid for the process lifetime; `ctx` is null on the
            // first call and our live context afterwards (SChannel updates it in place, as its
            // documentation allows); `target` is NUL-terminated; the input descriptor points at
            // `self.input`, which is not touched until the call returns; the output buffers are
            // allocated by SChannel (ISC_REQ_ALLOCATE_MEMORY) and freed below.
            let status = unsafe {
                InitializeSecurityContextW(
                    &self.cred,
                    if first { ptr::null() } else { ctx },
                    self.target.as_ptr(),
                    ISC_FLAGS,
                    0,
                    0,
                    if first { ptr::null() } else { &indesc },
                    0,
                    ctx,
                    &mut outdesc,
                    &mut attrs,
                    ptr::null_mut(),
                )
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
                    if !first {
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
                        return self.query_sizes();
                    }
                    if first {
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
                    // remaining input goes back through InitializeSecurityContextW.
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
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.has_ctx {
            // SAFETY: the context was created by InitializeSecurityContextW and is deleted once.
            unsafe { DeleteSecurityContext(&self.ctx) };
        }
    }
}
