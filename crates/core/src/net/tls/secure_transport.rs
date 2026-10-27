// PaTorrent's own OS glue, not ported (libtorrent uses OpenSSL through Boost.Asio).

//! TLS sessions on Secure Transport, the TLS library of macOS's Security.framework, and server
//! identities made from PEM files without the keychain.
//!
//! Secure Transport is the one Security.framework TLS API that works on buffers: a session
//! hands it read and write callbacks over memory, so it never touches the socket (Network.framework
//! insists on owning the connection). Apple deprecated it in macOS 10.15 and never gave it TLS
//! 1.3, so connections speak TLS 1.2 at most; it is still in every macOS and still the only
//! choice that fits the sans-IO [`TlsSession`](super::TlsSession). macOS 10.14 or later is
//! needed (`SecTrustEvaluateWithError`, `SecCertificateCopyKey`).
//!
//! A client stops at the server's certificate (`kSSLSessionOptionBreakOnServerAuth`) and checks
//! it itself: against the system's trust store with an SSL policy for the host name or IP
//! address and a soft-fail revocation policy ([`Check::System`], which uses no network for it,
//! and [`Check::SystemOnline`]), by fingerprint ([`Check::Pinned`]), or not at all
//! ([`Check::Any`]). SNI is sent for host names.
//!
//! A server identity needs a `SecIdentityRef`. The public way to get one wants the key in a
//! keychain; this backend makes the key in memory (`SecKeyCreateWithData`) and pairs it with
//! the certificate through `SecIdentityCreate`, which Security.framework exports without
//! declaring it in its headers (WebKit uses it the same way). It is looked up at run time, so a
//! macOS without it fails to load an identity instead of failing to start. Every function and
//! constant is declared here by hand, so no binding crate or build script is needed.

use std::ffi::{c_char, c_int, c_void};
use std::io;
use std::net::IpAddr;
use std::ptr;
use std::sync::{Arc, OnceLock};

use super::pem;
use super::{Check, Fingerprint, SubjectName};

type CFTypeRef = *const c_void;
type CFIndex = isize;
type OSStatus = i32;
type Boolean = u8;
type CFOptionFlags = usize;

type ReadFunc = unsafe extern "C" fn(CFTypeRef, *mut c_void, *mut usize) -> OSStatus;
type WriteFunc = unsafe extern "C" fn(CFTypeRef, *const c_void, *mut usize) -> OSStatus;
type SecIdentityCreateFn = unsafe extern "C" fn(CFTypeRef, CFTypeRef, CFTypeRef) -> CFTypeRef;

const NO_ERR: OSStatus = 0;
const ERR_SSL_WOULD_BLOCK: OSStatus = -9803;
const ERR_SSL_CLOSED_GRACEFUL: OSStatus = -9805;
const ERR_SSL_CLOSED_NO_NOTIFY: OSStatus = -9816;
const ERR_SSL_PEER_AUTH_COMPLETED: OSStatus = -9841;
const ERR_SSL_CLIENT_CERT_REQUESTED: OSStatus = -9842;

const K_SSL_SERVER_SIDE: c_int = 0;
const K_SSL_CLIENT_SIDE: c_int = 1;
const K_SSL_STREAM_TYPE: c_int = 0;
const K_SSL_SESSION_OPTION_BREAK_ON_SERVER_AUTH: c_int = 0;
const K_TLS_PROTOCOL_12: c_int = 8;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT32_TYPE: CFIndex = 3;
/// `kSecRevocationUseAnyAvailableMethod` (OCSP or CRL; soft-fail without
/// `kSecRevocationRequirePositiveResponse`) and `kSecRevocationNetworkAccessDisabled`.
const K_SEC_REVOCATION_USE_ANY_AVAILABLE_METHOD: CFOptionFlags = 3;
const K_SEC_REVOCATION_NETWORK_ACCESS_DISABLED: CFOptionFlags = 1 << 4;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    // The callback tables of collections of CF objects: only their addresses are passed on.
    static kCFTypeArrayCallBacks: u8;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    fn CFRelease(cf: CFTypeRef);
    fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, len: CFIndex) -> CFTypeRef;
    fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: CFIndex,
        encoding: u32,
        external: Boolean,
    ) -> CFTypeRef;
    fn CFStringGetLength(s: CFTypeRef) -> CFIndex;
    fn CFStringGetMaximumSizeForEncoding(len: CFIndex, encoding: u32) -> CFIndex;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: CFIndex, encoding: u32) -> Boolean;
    fn CFNumberCreate(alloc: CFTypeRef, kind: CFIndex, value: *const c_void) -> CFTypeRef;
    fn CFArrayCreate(
        alloc: CFTypeRef,
        values: *const CFTypeRef,
        len: CFIndex,
        callbacks: *const u8,
    ) -> CFTypeRef;
    fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        len: CFIndex,
        key_callbacks: *const u8,
        value_callbacks: *const u8,
    ) -> CFTypeRef;
    fn CFErrorCopyDescription(err: CFTypeRef) -> CFTypeRef;
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecAttrKeyType: CFTypeRef;
    static kSecAttrKeyTypeRSA: CFTypeRef;
    static kSecAttrKeyTypeECSECPrimeRandom: CFTypeRef;
    static kSecAttrKeyClass: CFTypeRef;
    static kSecAttrKeyClassPrivate: CFTypeRef;
    static kSecAttrKeySizeInBits: CFTypeRef;
    static kSecKeyAlgorithmECDSASignatureMessageX962SHA256: CFTypeRef;
    static kSecKeyAlgorithmRSASignatureMessagePKCS1v15SHA256: CFTypeRef;

    fn SSLCreateContext(alloc: CFTypeRef, side: c_int, kind: c_int) -> CFTypeRef;
    fn SSLSetIOFuncs(ctx: CFTypeRef, read: ReadFunc, write: WriteFunc) -> OSStatus;
    fn SSLSetConnection(ctx: CFTypeRef, connection: CFTypeRef) -> OSStatus;
    fn SSLSetPeerDomainName(ctx: CFTypeRef, name: *const c_char, len: usize) -> OSStatus;
    fn SSLSetSessionOption(ctx: CFTypeRef, option: c_int, value: Boolean) -> OSStatus;
    fn SSLSetProtocolVersionMin(ctx: CFTypeRef, version: c_int) -> OSStatus;
    fn SSLSetCertificate(ctx: CFTypeRef, certs: CFTypeRef) -> OSStatus;
    fn SSLHandshake(ctx: CFTypeRef) -> OSStatus;
    fn SSLRead(ctx: CFTypeRef, data: *mut c_void, len: usize, processed: *mut usize) -> OSStatus;
    fn SSLWrite(ctx: CFTypeRef, data: *const c_void, len: usize, processed: *mut usize) -> OSStatus;
    fn SSLCopyPeerTrust(ctx: CFTypeRef, trust: *mut CFTypeRef) -> OSStatus;

    fn SecCopyErrorMessageString(status: OSStatus, reserved: *mut c_void) -> CFTypeRef;
    fn SecPolicyCreateSSL(server: Boolean, hostname: CFTypeRef) -> CFTypeRef;
    fn SecPolicyCreateRevocation(flags: CFOptionFlags) -> CFTypeRef;
    fn SecTrustSetPolicies(trust: CFTypeRef, policies: CFTypeRef) -> OSStatus;
    fn SecTrustEvaluateWithError(trust: CFTypeRef, error: *mut CFTypeRef) -> bool;
    fn SecTrustGetCertificateAtIndex(trust: CFTypeRef, index: CFIndex) -> CFTypeRef;
    fn SecCertificateCreateWithData(alloc: CFTypeRef, data: CFTypeRef) -> CFTypeRef;
    fn SecCertificateCopyData(cert: CFTypeRef) -> CFTypeRef;
    fn SecCertificateCopyKey(cert: CFTypeRef) -> CFTypeRef;
    fn SecKeyCreateWithData(data: CFTypeRef, attributes: CFTypeRef, error: *mut CFTypeRef) -> CFTypeRef;
    fn SecKeyCreateRandomKey(parameters: CFTypeRef, error: *mut CFTypeRef) -> CFTypeRef;
    fn SecKeyCopyExternalRepresentation(key: CFTypeRef, error: *mut CFTypeRef) -> CFTypeRef;
    fn SecKeyCreateSignature(
        key: CFTypeRef,
        algorithm: CFTypeRef,
        data: CFTypeRef,
        error: *mut CFTypeRef,
    ) -> CFTypeRef;
    fn SecKeyVerifySignature(
        key: CFTypeRef,
        algorithm: CFTypeRef,
        data: CFTypeRef,
        signature: CFTypeRef,
        error: *mut CFTypeRef,
    ) -> Boolean;
}

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// An owned Core Foundation object (from a Create or Copy function), released on drop.
struct Cf(CFTypeRef);

impl Cf {
    /// Takes ownership of `p`; `None` when it is null. (Not `then_some(Cf(p))`: that builds the
    /// `Cf` either way, and dropping one of null calls `CFRelease(NULL)`, which traps.)
    fn new(p: CFTypeRef) -> Option<Cf> {
        if p.is_null() { None } else { Some(Cf(p)) }
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: `Cf` owns one reference to a live object and gives it back once.
        unsafe { CFRelease(self.0) };
    }
}

fn out_of_memory() -> io::Error {
    io::Error::other("out of memory")
}

fn data(bytes: &[u8]) -> io::Result<Cf> {
    // SAFETY: `bytes` is valid for its length; Core Foundation copies it.
    Cf::new(unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as CFIndex) })
        .ok_or_else(out_of_memory)
}

/// The bytes of a `CFData`.
fn bytes_of(d: &Cf) -> Vec<u8> {
    // SAFETY: `d` is a live CFData; its byte pointer is valid for its length while it lives.
    unsafe {
        let len = CFDataGetLength(d.0) as usize;
        if len == 0 {
            return Vec::new();
        }
        std::slice::from_raw_parts(CFDataGetBytePtr(d.0), len).to_vec()
    }
}

fn string(s: &str) -> io::Result<Cf> {
    // SAFETY: `s` is valid UTF-8 for its length; Core Foundation copies it.
    Cf::new(unsafe {
        CFStringCreateWithBytes(ptr::null(), s.as_ptr(), s.len() as CFIndex, K_CF_STRING_ENCODING_UTF8, 0)
    })
    .ok_or_else(out_of_memory)
}

/// A `CFString` as Rust text.
fn text(s: &Cf) -> String {
    // SAFETY: `s` is a live CFString; the buffer has room for the longest UTF-8 form plus the NUL.
    unsafe {
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s.0), K_CF_STRING_ENCODING_UTF8) + 1;
        let mut buf = vec![0u8; size.max(1) as usize];
        if CFStringGetCString(s.0, buf.as_mut_ptr().cast(), buf.len() as CFIndex, K_CF_STRING_ENCODING_UTF8)
            == 0
        {
            return String::new();
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }
}

/// The description of a `CFError` an out parameter received (and the reference released).
fn take_error(err: CFTypeRef) -> String {
    let Some(err) = Cf::new(err) else { return "unknown error".into() };
    // SAFETY: a live CFError.
    match Cf::new(unsafe { CFErrorCopyDescription(err.0) }) {
        Some(d) => text(&d),
        None => "unknown error".into(),
    }
}

fn error(what: &str, status: OSStatus) -> io::Error {
    // SAFETY: a plain lookup; the reserved argument must be null.
    let why = Cf::new(unsafe { SecCopyErrorMessageString(status, ptr::null_mut()) })
        .map(|s| text(&s))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Secure Transport error".into());
    io::Error::other(format!("{what}: {why} ({status})"))
}

/// A dictionary of CF keys and values (both retained by it).
fn dictionary(pairs: &[(CFTypeRef, CFTypeRef)]) -> io::Result<Cf> {
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    // SAFETY: both arrays hold `len` live CF objects; the callback tables are Core Foundation's.
    Cf::new(unsafe {
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            pairs.len() as CFIndex,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        )
    })
    .ok_or_else(out_of_memory)
}

// --- Sessions ------------------------------------------------------------------------------

/// The buffers Secure Transport's I/O callbacks read and write. The context holds a raw pointer
/// to it (from `Box::into_raw`), so it stays put while the session moves.
struct Io {
    /// Ciphertext from the peer; the callbacks have taken it up to `pos`.
    input: Vec<u8>,
    pos: usize,
    out: Vec<u8>,
}

/// Hands Secure Transport up to `*len` bytes of the received ciphertext; fewer, and
/// `errSSLWouldBlock`, when that is all there is.
unsafe extern "C" fn read_func(conn: CFTypeRef, data: *mut c_void, len: *mut usize) -> OSStatus {
    // SAFETY: `conn` is the session's `Io`, which only Secure Transport touches during the
    // session's own calls into it; `data` has room for `*len` bytes.
    unsafe {
        let io = &mut *(conn as *mut Io);
        let want = *len;
        let n = want.min(io.input.len() - io.pos);
        ptr::copy_nonoverlapping(io.input.as_ptr().add(io.pos), data.cast::<u8>(), n);
        io.pos += n;
        *len = n;
        if n < want { ERR_SSL_WOULD_BLOCK } else { NO_ERR }
    }
}

/// Takes everything Secure Transport writes.
unsafe extern "C" fn write_func(conn: CFTypeRef, data: *const c_void, len: *mut usize) -> OSStatus {
    // SAFETY: as in `read_func`; `data` holds `*len` bytes.
    unsafe {
        let io = &mut *(conn as *mut Io);
        io.out.extend_from_slice(std::slice::from_raw_parts(data.cast::<u8>(), *len));
        NO_ERR
    }
}

pub(crate) struct Session {
    /// The `SSLContextRef`, owned.
    ctx: CFTypeRef,
    /// From `Box::into_raw`, freed in Drop after the context.
    io: *mut Io,
    /// A client's host name or address and how it checks the server.
    client: Option<(String, Check)>,
    handshaking: bool,
    /// Whether the peer's first bytes were seen to be a TLS record.
    started: bool,
    closed: bool,
    /// The server's certificate, as a client saw it.
    peer: Option<Fingerprint>,
    /// A client checked the server's certificate. A handshake that ends without the
    /// server-auth break never checked it, and fails (SEC-07).
    verified: bool,
}

// SAFETY: the context and the buffers belong to this session alone and are only used through
// `&mut self`, so moving the session to another thread is fine.
unsafe impl Send for Session {}

impl Session {
    fn new(side: c_int) -> io::Result<Session> {
        // SAFETY: a new context we own (released in Drop).
        let ctx = unsafe { SSLCreateContext(ptr::null(), side, K_SSL_STREAM_TYPE) };
        if ctx.is_null() {
            return Err(io::Error::other("TLS is not available: SSLCreateContext failed"));
        }
        let io = Box::into_raw(Box::new(Io { input: Vec::new(), pos: 0, out: Vec::new() }));
        // from here on Drop frees both
        let s = Session {
            ctx,
            io,
            client: None,
            handshaking: true,
            started: false,
            closed: false,
            peer: None,
            verified: false,
        };
        // SAFETY: the context is live; the connection pointer stays valid until Drop releases
        // the context, and the callbacks treat it as the `Io` it is.
        let mut status = unsafe { SSLSetIOFuncs(ctx, read_func, write_func) };
        if status == NO_ERR {
            // SAFETY: as above.
            status = unsafe { SSLSetConnection(ctx, io as CFTypeRef) };
        }
        if status != NO_ERR {
            return Err(error("TLS setup failed", status));
        }
        Ok(s)
    }

    pub fn client(host: &str, check: Check) -> io::Result<Session> {
        if host.contains('\0') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid host name"));
        }
        let mut s = Session::new(K_SSL_CLIENT_SIDE)?;
        // SAFETY: the context is live and not started; Secure Transport copies the name.
        let mut status = unsafe { SSLSetSessionOption(s.ctx, K_SSL_SESSION_OPTION_BREAK_ON_SERVER_AUTH, 1) };
        // TLS 1.2 and later: 1.0 and 1.1 are deprecated (RFC 8996, SEC-07)
        if status == NO_ERR {
            // SAFETY: as above.
            status = unsafe { SSLSetProtocolVersionMin(s.ctx, K_TLS_PROTOCOL_12) };
        }
        // SNI names hosts, never addresses (RFC 6066)
        if status == NO_ERR && host.parse::<IpAddr>().is_err() {
            // SAFETY: as above; `host` is valid for its length.
            status = unsafe { SSLSetPeerDomainName(s.ctx, host.as_ptr().cast(), host.len()) };
        }
        if status != NO_ERR {
            return Err(error("TLS setup failed", status));
        }
        s.client = Some((host.to_owned(), check));
        s.handshake()?;
        Ok(s)
    }

    pub fn server(identity: &Arc<Identity>) -> io::Result<Session> {
        let s = Session::new(K_SSL_SERVER_SIDE)?;
        // SAFETY: the context is live and not started; it retains the identity's array.
        let mut status = unsafe { SSLSetProtocolVersionMin(s.ctx, K_TLS_PROTOCOL_12) };
        if status == NO_ERR {
            // SAFETY: as above.
            status = unsafe { SSLSetCertificate(s.ctx, identity.chain.0) };
        }
        if status != NO_ERR {
            return Err(error("TLS setup failed", status));
        }
        Ok(s)
    }

    fn io(&mut self) -> &mut Io {
        // SAFETY: the box lives until Drop; Secure Transport only reaches it during our calls
        // into the context, none of which runs while this borrow is held.
        unsafe { &mut *self.io }
    }

    /// Runs the handshake over the buffered input until it needs more data. A client's first
    /// call produces the ClientHello from nothing.
    fn handshake(&mut self) -> io::Result<()> {
        loop {
            // SAFETY: the context is live; the callbacks use the session's `Io` only.
            match unsafe { SSLHandshake(self.ctx) } {
                NO_ERR => {
                    if self.client.is_some() && !self.verified {
                        return Err(io::Error::other(
                            "TLS handshake failed: the server's certificate was not checked",
                        ));
                    }
                    self.handshaking = false;
                    return Ok(());
                }
                ERR_SSL_WOULD_BLOCK => return Ok(()),
                ERR_SSL_PEER_AUTH_COMPLETED => {
                    self.check_server()?;
                    self.verified = true;
                }
                // a server asking for a client certificate gets none
                ERR_SSL_CLIENT_CERT_REQUESTED => {}
                e => return Err(error("TLS handshake failed", e)),
            }
        }
    }

    /// A client notes the server's certificate and checks it as it was told to.
    fn check_server(&mut self) -> io::Result<()> {
        let Some((host, check)) = self.client.clone() else { return Ok(()) };
        let mut trust: CFTypeRef = ptr::null();
        // SAFETY: the context is live and at the server-auth break; we own the trust we get.
        let status = unsafe { SSLCopyPeerTrust(self.ctx, &mut trust) };
        let trust = Cf::new(trust);
        let Some(trust) = trust.filter(|_| status == NO_ERR) else {
            return Err(io::Error::other("TLS handshake failed: the server presented no certificate"));
        };
        // SAFETY: a Get function: the certificate belongs to the trust, which outlives its use.
        let leaf = unsafe { SecTrustGetCertificateAtIndex(trust.0, 0) };
        // SAFETY: as above; we own the copy.
        let der = (!leaf.is_null()).then(|| Cf::new(unsafe { SecCertificateCopyData(leaf) })).flatten();
        self.peer = der.map(|d| Fingerprint::of(&bytes_of(&d)));
        match check {
            Check::Any => Ok(()),
            Check::Pinned(pin) if self.peer == Some(pin) => Ok(()),
            Check::Pinned(_) => Err(io::Error::other(format!(
                "TLS handshake failed: the server's certificate is not the accepted one (it presented {})",
                self.peer.map_or_else(|| "none".to_owned(), |p| p.to_string())
            ))),
            Check::System | Check::SystemOnline => {
                let name = string(&host)?;
                // SAFETY: `name` is a live CFString; we own the policy.
                let ssl = Cf::new(unsafe { SecPolicyCreateSSL(1, name.0) })
                    .ok_or_else(|| io::Error::other("TLS handshake failed: no SSL policy"))?;
                // soft-fail revocation (SEC-07); online only where a lookup cannot hold up the
                // engine thread or go around the proxy, as on SChannel
                let mut flags = K_SEC_REVOCATION_USE_ANY_AVAILABLE_METHOD;
                if check == Check::System {
                    flags |= K_SEC_REVOCATION_NETWORK_ACCESS_DISABLED;
                }
                // SAFETY: no arguments but the flags; we own the policy.
                let revocation = Cf::new(unsafe { SecPolicyCreateRevocation(flags) })
                    .ok_or_else(|| io::Error::other("TLS handshake failed: no revocation policy"))?;
                let refs = [ssl.0, revocation.0];
                // SAFETY: `refs` holds live policies, which the array retains.
                let policies = Cf::new(unsafe {
                    CFArrayCreate(
                        ptr::null(),
                        refs.as_ptr(),
                        refs.len() as CFIndex,
                        &raw const kCFTypeArrayCallBacks,
                    )
                })
                .ok_or_else(out_of_memory)?;
                // SAFETY: both are live; the trust retains the policies.
                let status = unsafe { SecTrustSetPolicies(trust.0, policies.0) };
                if status != NO_ERR {
                    return Err(error("TLS handshake failed", status));
                }
                let mut err: CFTypeRef = ptr::null();
                // SAFETY: the trust is live; an error, if any, is ours to release.
                if unsafe { SecTrustEvaluateWithError(trust.0, &mut err) } {
                    Ok(())
                } else {
                    Err(io::Error::other(format!(
                        "TLS handshake failed: certificate verification failed: {}",
                        take_error(err)
                    )))
                }
            }
        }
    }

    /// The handshake, then every record the buffered input completes.
    fn process(&mut self, plain: &mut Vec<u8>) -> io::Result<()> {
        if self.handshaking {
            self.handshake()?;
            if self.handshaking {
                return Ok(());
            }
        }
        let mut buf = [0u8; 16 * 1024];
        loop {
            let mut n = 0;
            // SAFETY: the context is live; `buf` has room for `buf.len()` bytes.
            let status = unsafe { SSLRead(self.ctx, buf.as_mut_ptr().cast(), buf.len(), &mut n) };
            plain.extend_from_slice(&buf[..n.min(buf.len())]);
            match status {
                NO_ERR if n > 0 => {}
                NO_ERR | ERR_SSL_WOULD_BLOCK => return Ok(()),
                ERR_SSL_CLOSED_GRACEFUL | ERR_SSL_CLOSED_NO_NOTIFY => {
                    self.closed = true;
                    return Ok(());
                }
                e => return Err(error("TLS receive failed", e)),
            }
        }
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
        let input = &mut self.io().input;
        input.extend_from_slice(ciphertext);
        // a plain-text request reads as the header of a long record, which the handshake would
        // wait for (SChannel does); a peer's first record is a handshake record (22) or an alert
        // (21)
        let tls = matches!(input.first(), Some(21 | 22)) && input.get(1).is_none_or(|&v| v == 3);
        let empty = input.is_empty();
        if !self.started && !empty {
            if !tls {
                return Err(io::Error::other("TLS handshake failed: the peer does not speak TLS"));
            }
            self.started = true;
        }
        let r = self.process(plain);
        let io = self.io();
        let used = io.pos;
        io.input.drain(..used);
        io.pos = 0;
        r
    }

    fn encrypt(&mut self, plain: &[u8]) -> io::Result<()> {
        debug_assert!(!self.handshaking);
        for chunk in plain.chunks(16 * 1024) {
            let mut n = 0;
            // SAFETY: the context is live; `chunk` is valid for its length. The write callback
            // takes everything, so the whole chunk goes or the call fails.
            let status = unsafe { SSLWrite(self.ctx, chunk.as_ptr().cast(), chunk.len(), &mut n) };
            if status != NO_ERR || n != chunk.len() {
                return Err(error("TLS send failed", status));
            }
        }
        Ok(())
    }

    fn outgoing(&mut self) -> &mut Vec<u8> {
        &mut self.io().out
    }

    fn closed(&self) -> bool {
        self.closed
    }

    fn peer_fingerprint(&self) -> Option<Fingerprint> {
        if self.handshaking { None } else { self.peer }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the context is released once, and only then the buffers its connection
        // pointer referred to.
        unsafe {
            CFRelease(self.ctx);
            drop(Box::from_raw(self.io));
        }
    }
}

// --- Server identities ---------------------------------------------------------------------

/// The array `SSLSetCertificate` takes: the identity (certificate and key), then the rest of
/// the chain.
pub(crate) struct Identity {
    chain: Cf,
}

// SAFETY: the array and what it holds are immutable after construction; Core Foundation allows
// such objects to be used from any thread, and sessions only retain it.
unsafe impl Send for Identity {}
// SAFETY: as above.
unsafe impl Sync for Identity {}

#[derive(Clone, Copy)]
enum KeyKind {
    Rsa,
    Ec,
}

impl KeyKind {
    fn signature_algorithm(self) -> CFTypeRef {
        // SAFETY: Security.framework's constant strings.
        unsafe {
            match self {
                KeyKind::Rsa => kSecKeyAlgorithmRSASignatureMessagePKCS1v15SHA256,
                KeyKind::Ec => kSecKeyAlgorithmECDSASignatureMessageX962SHA256,
            }
        }
    }
}

fn does_not_belong() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "the private key does not belong to the certificate")
}

fn certificate(der: &[u8], what: &str) -> io::Result<Cf> {
    let d = data(der)?;
    // SAFETY: `d` is a live CFData; we own the certificate, null when the DER is not one.
    Cf::new(unsafe { SecCertificateCreateWithData(ptr::null(), d.0) })
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, what.to_owned()))
}

/// `SecIdentityCreate`, if this macOS exports it.
fn sec_identity_create() -> Option<SecIdentityCreateFn> {
    static F: OnceLock<Option<SecIdentityCreateFn>> = OnceLock::new();
    *F.get_or_init(|| {
        // SAFETY: a lookup in every loaded image (RTLD_DEFAULT is -2 on Apple systems) of a
        // NUL-terminated name; Security.framework is linked, so it is loaded.
        let p = unsafe { dlsym(ptr::without_provenance_mut(usize::MAX - 1), c"SecIdentityCreate".as_ptr()) };
        // SAFETY: the symbol is Security.framework's `SecIdentityCreate(CFAllocatorRef,
        // SecCertificateRef, SecKeyRef)`, which has this signature.
        (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, SecIdentityCreateFn>(p) })
    })
}

/// The private key in the form `SecKeyCreateWithData` reads: PKCS #1 for RSA, `04 || X || Y ||
/// D` for EC (the public point from the key, or else from the certificate).
fn raw_private_key(key: &pem::Block, cert: &[u8]) -> io::Result<(KeyKind, Vec<u8>)> {
    let corrupt = || io::Error::new(io::ErrorKind::InvalidData, "the private key is corrupt");
    let (kind, der) = match key.label.as_str() {
        "RSA PRIVATE KEY" => (KeyKind::Rsa, &key.der[..]),
        "EC PRIVATE KEY" => (KeyKind::Ec, &key.der[..]),
        _ => match pem::pkcs8_key(&key.der).ok_or_else(corrupt)? {
            (pem::RSA_OID, inner) => (KeyKind::Rsa, inner),
            (pem::EC_OID, inner) => (KeyKind::Ec, inner),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported private key type (RSA and EC keys are supported)",
                ));
            }
        },
    };
    match kind {
        KeyKind::Rsa => Ok((kind, der.to_vec())),
        KeyKind::Ec => {
            let (d, public) = pem::ec_private_key(der).ok_or_else(corrupt)?;
            let point = public
                .or_else(|| pem::subject_public_key(cert))
                .filter(|p| p.len() % 2 == 1 && p[0] == 4)
                .ok_or_else(does_not_belong)?;
            // the scalar as long as a coordinate, in case the file dropped leading zeros
            let field = (point.len() - 1) / 2;
            if d.len() > field {
                return Err(does_not_belong());
            }
            let mut raw = point.to_vec();
            raw.resize(point.len() + field - d.len(), 0);
            raw.extend_from_slice(d);
            Ok((kind, raw))
        }
    }
}

/// Whether `key` signs what the certificate's public key verifies.
fn signs_for(key: &Cf, cert: &Cf, kind: KeyKind) -> io::Result<bool> {
    // SAFETY: a live certificate; we own its key (null when Security cannot read it).
    let Some(public) = Cf::new(unsafe { SecCertificateCopyKey(cert.0) }) else { return Ok(false) };
    let message = data(b"PaTorrent checks that the key belongs to the certificate")?;
    let algorithm = kind.signature_algorithm();
    let mut err: CFTypeRef = ptr::null();
    // SAFETY: live key and data; we own the signature and any error.
    let signature = Cf::new(unsafe { SecKeyCreateSignature(key.0, algorithm, message.0, &mut err) });
    let Some(signature) = signature else {
        return Err(io::Error::other(format!("the private key cannot sign: {}", take_error(err))));
    };
    let mut err: CFTypeRef = ptr::null();
    // SAFETY: as above; a key of another type fails with an error, which we release.
    let ok = unsafe { SecKeyVerifySignature(public.0, algorithm, message.0, signature.0, &mut err) } != 0;
    drop(Cf::new(err));
    Ok(ok)
}

impl Identity {
    pub fn new(certs: &[Vec<u8>], key: &pem::Block) -> io::Result<Identity> {
        let leaf = certificate(&certs[0], "the certificate cannot be read")?;
        let (kind, raw) = raw_private_key(key, &certs[0])?;
        let raw = data(&raw)?;
        // SAFETY: Security.framework's constant strings.
        let (key_type, class, private) =
            unsafe { (kSecAttrKeyType, kSecAttrKeyClass, kSecAttrKeyClassPrivate) };
        // SAFETY: as above.
        let algorithm = unsafe {
            match kind {
                KeyKind::Rsa => kSecAttrKeyTypeRSA,
                KeyKind::Ec => kSecAttrKeyTypeECSECPrimeRandom,
            }
        };
        let attributes = dictionary(&[(key_type, algorithm), (class, private)])?;
        let mut err: CFTypeRef = ptr::null();
        // SAFETY: live data and attributes; the key lives in memory only (no keychain), ours.
        let secret = Cf::new(unsafe { SecKeyCreateWithData(raw.0, attributes.0, &mut err) });
        let Some(secret) = secret else {
            let why = take_error(err);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the private key cannot be read: {why}"),
            ));
        };
        if !signs_for(&secret, &leaf, kind)? {
            return Err(does_not_belong());
        }
        let create = sec_identity_create().ok_or_else(|| {
            io::Error::other("this macOS cannot make a TLS identity (no SecIdentityCreate)")
        })?;
        // SAFETY: a live certificate and key; we own the identity.
        let identity = Cf::new(unsafe { create(ptr::null(), leaf.0, secret.0) })
            .ok_or_else(|| io::Error::other("the certificate and key cannot be paired"))?;
        let mut items = vec![identity];
        for der in &certs[1..] {
            items.push(certificate(der, "a certificate of the chain cannot be read")?);
        }
        let refs: Vec<CFTypeRef> = items.iter().map(|c| c.0).collect();
        // SAFETY: `refs` holds live CF objects, which the array retains.
        let chain = Cf::new(unsafe {
            CFArrayCreate(ptr::null(), refs.as_ptr(), refs.len() as CFIndex, &raw const kCFTypeArrayCallBacks)
        })
        .ok_or_else(out_of_memory)?;
        Ok(Identity { chain })
    }
}

// --- Self-signed certificates --------------------------------------------------------------

/// See [`super::generate_self_signed`]: a P-256 key made in memory by Security.framework, which
/// also signs the certificate; the certificate itself is built here (`pem::self_signed_tbs`),
/// as macOS has no public call that makes one.
pub(crate) fn generate_self_signed(
    cn: &str,
    names: &[SubjectName],
    not_before: i64,
    not_after: i64,
) -> io::Result<(String, String)> {
    let bits: i32 = 256;
    // SAFETY: `bits` is a valid 32-bit integer for the call; we own the number.
    let bits =
        Cf::new(unsafe { CFNumberCreate(ptr::null(), K_CF_NUMBER_SINT32_TYPE, (&raw const bits).cast()) })
            .ok_or_else(out_of_memory)?;
    // SAFETY: Security.framework's constant strings.
    let (key_type, ec, size) =
        unsafe { (kSecAttrKeyType, kSecAttrKeyTypeECSECPrimeRandom, kSecAttrKeySizeInBits) };
    let params = dictionary(&[(key_type, ec), (size, bits.0)])?;
    let mut err: CFTypeRef = ptr::null();
    // SAFETY: live parameters; the key is not permanent (in memory only) and ours.
    let key = Cf::new(unsafe { SecKeyCreateRandomKey(params.0, &mut err) })
        .ok_or_else(|| io::Error::other(format!("cannot create a key: {}", take_error(err))))?;
    let mut err: CFTypeRef = ptr::null();
    // SAFETY: a live key made exportable by default; we own the data.
    let raw = Cf::new(unsafe { SecKeyCopyExternalRepresentation(key.0, &mut err) })
        .ok_or_else(|| io::Error::other(format!("cannot export the new key: {}", take_error(err))))?;
    // 04 || X || Y || D
    let raw = bytes_of(&raw);
    if raw.len() != 97 || raw[0] != 4 {
        return Err(io::Error::other("cannot export the new key: unexpected format"));
    }
    let (point, d) = raw.split_at(65);
    let mut serial = [0u8; 8];
    crate::util::random::crypto_random_bytes(&mut serial);
    // positive, non-zero and minimal: the top bit clear, the next one set
    serial[0] = (serial[0] & 0x7f) | 0x40;
    let tbs = pem::self_signed_tbs(cn, names, &serial, not_before, not_after, point);
    let tbs_data = data(&tbs)?;
    let mut err: CFTypeRef = ptr::null();
    // SAFETY: a live key and data; we own the signature (a DER Ecdsa-Sig-Value).
    let signature = Cf::new(unsafe {
        SecKeyCreateSignature(key.0, KeyKind::Ec.signature_algorithm(), tbs_data.0, &mut err)
    })
    .ok_or_else(|| io::Error::other(format!("cannot sign the certificate: {}", take_error(err))))?;
    let cert = pem::self_signed_certificate(&tbs, &bytes_of(&signature));
    Ok((pem::encode("CERTIFICATE", &cert), pem::encode("PRIVATE KEY", &pem::p256_pkcs8(d, point))))
}
