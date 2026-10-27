// PaTorrent's own OS glue, not ported (libtorrent uses OpenSSL through Boost.Asio).

//! TLS sessions on the system's OpenSSL (1.1.1 or later) or LibreSSL.
//!
//! A session talks to OpenSSL through two memory BIOs, so it never touches the socket. A
//! client verifies its peer (`SSL_VERIFY_PEER` against the default trust store), the
//! certificate must match the host name (`SSL_set1_host`) or IP address, and SNI is sent for
//! host names; a pinned client accepts any chain and compares the fingerprint of the server's
//! certificate instead, which a verify callback records. Revocation is not checked (SEC-07):
//! OpenSSL has no CRL or OCSP source of its own, and fetching them here would block the engine
//! thread, so this back end does what most Linux programs do. A server context holds the
//! certificate, its chain and the key. Only functions present in OpenSSL 1.1.1, 3.x and LibreSSL
//! are used, declared here by hand so no binding crate or build script is needed; linking needs
//! `libssl` and `libcrypto`.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_ulong, c_void};
use std::io;
use std::net::IpAddr;
use std::ptr;
use std::sync::{Arc, OnceLock};

use super::pem;
use super::{Check, Fingerprint, SubjectName};

type Handle = *mut c_void;
type VerifyCallback = unsafe extern "C" fn(c_int, Handle) -> c_int;

const SSL_VERIFY_PEER: c_int = 1;
const SSL_ERROR_WANT_READ: c_int = 2;
const SSL_ERROR_WANT_WRITE: c_int = 3;
const SSL_ERROR_ZERO_RETURN: c_int = 6;
const SSL_CTRL_EXTRA_CHAIN_CERT: c_int = 14;
const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;
const TLS1_2_VERSION: c_long = 0x0303;
const TLSEXT_NAMETYPE_HOST_NAME: c_long = 0;
const X509_V_OK: c_long = 0;
const NID_X9_62_PRIME256V1: c_int = 415;
const NID_SUBJECT_ALT_NAME: c_int = 85;
const NID_EXT_KEY_USAGE: c_int = 126;
const MBSTRING_UTF8: c_int = 0x1000;

#[link(name = "ssl")]
#[link(name = "crypto")]
unsafe extern "C" {
    fn TLS_client_method() -> *const c_void;
    fn TLS_server_method() -> *const c_void;
    fn SSL_CTX_new(method: *const c_void) -> Handle;
    fn SSL_CTX_free(ctx: Handle);
    fn SSL_CTX_ctrl(ctx: Handle, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn SSL_CTX_set_default_verify_paths(ctx: Handle) -> c_int;
    fn SSL_CTX_set_verify(ctx: Handle, mode: c_int, callback: *const c_void);
    fn SSL_CTX_use_certificate(ctx: Handle, x509: Handle) -> c_int;
    fn SSL_CTX_use_PrivateKey(ctx: Handle, pkey: Handle) -> c_int;
    fn SSL_CTX_check_private_key(ctx: Handle) -> c_int;
    fn SSL_new(ctx: Handle) -> Handle;
    fn SSL_free(ssl: Handle);
    fn SSL_set_bio(ssl: Handle, rbio: Handle, wbio: Handle);
    fn SSL_set_connect_state(ssl: Handle);
    fn SSL_set_accept_state(ssl: Handle);
    fn SSL_set_verify(ssl: Handle, mode: c_int, callback: Option<VerifyCallback>);
    fn SSL_set_ex_data(ssl: Handle, idx: c_int, data: *mut c_void) -> c_int;
    fn SSL_get_ex_data(ssl: Handle, idx: c_int) -> *mut c_void;
    fn SSL_get_ex_data_X509_STORE_CTX_idx() -> c_int;
    fn SSL_ctrl(ssl: Handle, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn SSL_set1_host(ssl: Handle, hostname: *const c_char) -> c_int;
    fn SSL_get0_param(ssl: Handle) -> Handle;
    fn X509_VERIFY_PARAM_set1_ip_asc(param: Handle, ipasc: *const c_char) -> c_int;
    fn SSL_do_handshake(ssl: Handle) -> c_int;
    fn SSL_read(ssl: Handle, buf: *mut c_void, num: c_int) -> c_int;
    fn SSL_write(ssl: Handle, buf: *const c_void, num: c_int) -> c_int;
    fn SSL_get_error(ssl: Handle, ret: c_int) -> c_int;
    fn SSL_get_verify_result(ssl: Handle) -> c_long;
    fn X509_verify_cert_error_string(n: c_long) -> *const c_char;
    fn X509_STORE_CTX_get_ex_data(ctx: Handle, idx: c_int) -> *mut c_void;
    fn X509_STORE_CTX_get_current_cert(ctx: Handle) -> Handle;
    fn X509_STORE_CTX_get_error_depth(ctx: Handle) -> c_int;
    fn ERR_get_error() -> c_ulong;
    fn ERR_error_string_n(e: c_ulong, buf: *mut c_char, len: usize);
    fn ERR_clear_error();
    fn BIO_new(method: *const c_void) -> Handle;
    fn BIO_s_mem() -> *const c_void;
    fn BIO_free(bio: Handle) -> c_int;
    fn BIO_read(bio: Handle, data: *mut c_void, len: c_int) -> c_int;
    fn BIO_write(bio: Handle, data: *const c_void, len: c_int) -> c_int;
    fn d2i_X509(out: *mut Handle, input: *mut *const u8, len: c_long) -> Handle;
    fn i2d_X509(x509: Handle, out: *mut *mut u8) -> c_int;
    fn d2i_AutoPrivateKey(out: *mut Handle, input: *mut *const u8, len: c_long) -> Handle;
    fn X509_free(x509: Handle);
    fn EVP_PKEY_new() -> Handle;
    fn EVP_PKEY_free(pkey: Handle);
    fn EVP_PKEY_set1_EC_KEY(pkey: Handle, key: Handle) -> c_int;
    fn EC_KEY_new_by_curve_name(nid: c_int) -> Handle;
    fn EC_KEY_generate_key(key: Handle) -> c_int;
    fn EC_KEY_free(key: Handle);
    fn X509_new() -> Handle;
    fn X509_set_version(x509: Handle, version: c_long) -> c_int;
    fn X509_get_serialNumber(x509: Handle) -> Handle;
    fn ASN1_INTEGER_set(a: Handle, v: c_long) -> c_int;
    fn X509_getm_notBefore(x509: Handle) -> Handle;
    fn X509_getm_notAfter(x509: Handle) -> Handle;
    fn ASN1_TIME_set(s: Handle, t: i64) -> Handle;
    fn X509_set_pubkey(x509: Handle, pkey: Handle) -> c_int;
    fn X509_get_subject_name(x509: Handle) -> Handle;
    fn X509_set_issuer_name(x509: Handle, name: Handle) -> c_int;
    fn X509_NAME_add_entry_by_txt(
        name: Handle,
        field: *const c_char,
        kind: c_int,
        bytes: *const u8,
        len: c_int,
        loc: c_int,
        set: c_int,
    ) -> c_int;
    fn X509V3_EXT_conf_nid(conf: Handle, ctx: Handle, nid: c_int, value: *const c_char) -> Handle;
    fn X509_add_ext(x509: Handle, ext: Handle, loc: c_int) -> c_int;
    fn X509_EXTENSION_free(ext: Handle);
    fn EVP_sha256() -> *const c_void;
    fn X509_sign(x509: Handle, pkey: Handle, md: *const c_void) -> c_int;
    fn PEM_write_bio_X509(bio: Handle, x509: Handle) -> c_int;
    fn PEM_write_bio_PKCS8PrivateKey(
        bio: Handle,
        pkey: Handle,
        cipher: *const c_void,
        pass: *const c_char,
        len: c_int,
        cb: *const c_void,
        u: *mut c_void,
    ) -> c_int;
}

struct Context(Handle);

// SAFETY: the SSL_CTX is fully configured before it is shared and only used read-only afterwards
// (SSL_new), which OpenSSL allows from any thread.
unsafe impl Send for Context {}
// SAFETY: as above.
unsafe impl Sync for Context {}

/// The process-wide client context: default trust store, peer verification on.
fn context() -> io::Result<Handle> {
    static CTX: OnceLock<Result<Context, String>> = OnceLock::new();
    let ctx = CTX.get_or_init(|| {
        // SAFETY: plain OpenSSL calls on a context we just created and own; a null callback
        // keeps OpenSSL's default verification.
        unsafe {
            ERR_clear_error();
            let ctx = SSL_CTX_new(TLS_client_method());
            if ctx.is_null() {
                return Err(last_error());
            }
            if SSL_CTX_set_default_verify_paths(ctx) != 1 {
                return Err(last_error());
            }
            SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, ptr::null());
            // TLS 1.2 and later: 1.0 and 1.1 are deprecated (RFC 8996, SEC-07)
            if SSL_CTX_ctrl(ctx, SSL_CTRL_SET_MIN_PROTO_VERSION, TLS1_2_VERSION, ptr::null_mut()) != 1 {
                return Err(last_error());
            }
            Ok(Context(ctx))
        }
    });
    match ctx {
        Ok(c) => Ok(c.0),
        Err(e) => Err(io::Error::other(format!("TLS is not available: {e}"))),
    }
}

/// The oldest error on this thread's OpenSSL error queue, as text.
fn last_error() -> String {
    // SAFETY: ERR_get_error has no preconditions; ERR_error_string_n writes at most `buf.len()`
    // bytes including the NUL terminator.
    unsafe {
        let code = ERR_get_error();
        if code == 0 {
            return "unknown OpenSSL error".into();
        }
        let mut buf = [0 as c_char; 256];
        ERR_error_string_n(code, buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

fn failed(what: &str) -> io::Error {
    io::Error::other(format!("{what}: {}", last_error()))
}

/// What a client's verify callback found; boxed, so its address (the SSL object's app data)
/// stays put while the session moves.
struct Verification {
    pinned: bool,
    leaf: Option<Fingerprint>,
}

/// Records the fingerprint of the server's own certificate (depth 0); a pinned session accepts
/// every chain here and compares the fingerprint after the handshake.
unsafe extern "C" fn verify_callback(preverify_ok: c_int, store: Handle) -> c_int {
    // SAFETY: OpenSSL calls this during a handshake of an SSL object whose app data (index 0)
    // is our live `Verification` (set before the handshake, cleared never: the box lives as
    // long as the SSL object). The store context and its current certificate are valid for
    // the call.
    unsafe {
        let ssl = X509_STORE_CTX_get_ex_data(store, SSL_get_ex_data_X509_STORE_CTX_idx());
        let state = SSL_get_ex_data(ssl, 0) as *mut Verification;
        if state.is_null() {
            return preverify_ok;
        }
        if X509_STORE_CTX_get_error_depth(store) == 0 {
            let cert = X509_STORE_CTX_get_current_cert(store);
            let len = i2d_X509(cert, ptr::null_mut());
            if len > 0 {
                let mut der = vec![0u8; len as usize];
                let mut p = der.as_mut_ptr();
                if i2d_X509(cert, &mut p) == len {
                    (*state).leaf = Some(Fingerprint::of(&der));
                }
            }
        }
        if (*state).pinned { 1 } else { preverify_ok }
    }
}

pub(crate) struct Session {
    ssl: Handle,
    /// Network → OpenSSL; owned by `ssl`.
    rbio: Handle,
    /// OpenSSL → network; owned by `ssl`.
    wbio: Handle,
    out: Vec<u8>,
    handshaking: bool,
    closed: bool,
    /// A client's verification state (the SSL object points at it).
    verification: Option<Box<Verification>>,
    check: Check,
}

// SAFETY: the SSL object and its BIOs belong to this session alone and are only used through
// `&mut self`, so moving the session to another thread is fine.
unsafe impl Send for Session {}

impl Session {
    /// An SSL object on `ctx` with two memory BIOs.
    fn new(ctx: Handle) -> io::Result<Session> {
        // SAFETY: `ctx` is a live context. The session frees the SSL object on drop; once
        // SSL_set_bio has run, the SSL object owns both BIOs and frees them with itself.
        unsafe {
            ERR_clear_error();
            let ssl = SSL_new(ctx);
            if ssl.is_null() {
                return Err(failed("TLS setup failed"));
            }
            let mut s = Session {
                ssl,
                rbio: ptr::null_mut(),
                wbio: ptr::null_mut(),
                out: Vec::new(),
                handshaking: true,
                closed: false,
                verification: None,
                check: Check::System,
            };
            let rbio = BIO_new(BIO_s_mem());
            let wbio = BIO_new(BIO_s_mem());
            if rbio.is_null() || wbio.is_null() {
                for b in [rbio, wbio] {
                    if !b.is_null() {
                        BIO_free(b);
                    }
                }
                return Err(io::Error::other("TLS setup failed: out of memory"));
            }
            SSL_set_bio(ssl, rbio, wbio);
            (s.rbio, s.wbio) = (rbio, wbio);
            Ok(s)
        }
    }

    pub fn client(host: &str, check: Check) -> io::Result<Session> {
        let name = CString::new(host)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid host name"))?;
        let mut s = Session::new(context()?)?;
        s.check = check;
        let mut state = Box::new(Verification {
            pinned: !matches!(check, Check::System | Check::SystemOnline),
            leaf: None,
        });
        let is_ip = host.parse::<IpAddr>().is_ok();
        // SAFETY: `s.ssl` is live and `name` is NUL-terminated (OpenSSL copies it). The app
        // data points into the box, which the session keeps as long as the SSL object.
        let ok = unsafe {
            SSL_set_connect_state(s.ssl);
            SSL_set_verify(s.ssl, SSL_VERIFY_PEER, Some(verify_callback));
            let mut ok = SSL_set_ex_data(s.ssl, 0, (&raw mut *state).cast()) == 1;
            if !is_ip {
                ok &= SSL_ctrl(
                    s.ssl,
                    SSL_CTRL_SET_TLSEXT_HOSTNAME,
                    TLSEXT_NAMETYPE_HOST_NAME,
                    name.as_ptr() as *mut c_void,
                ) == 1;
            }
            // a pinned certificate need not name the host
            if matches!(s.check, Check::System | Check::SystemOnline) {
                ok &= if is_ip {
                    X509_VERIFY_PARAM_set1_ip_asc(SSL_get0_param(s.ssl), name.as_ptr()) == 1
                } else {
                    SSL_set1_host(s.ssl, name.as_ptr()) == 1
                };
            }
            ok
        };
        s.verification = Some(state);
        if !ok {
            return Err(failed("TLS setup failed"));
        }
        s.handshake()?;
        Ok(s)
    }

    pub fn server(identity: &Arc<Identity>) -> io::Result<Session> {
        let s = Session::new(identity.ctx)?;
        // SAFETY: `s.ssl` is live.
        unsafe { SSL_set_accept_state(s.ssl) };
        Ok(s)
    }

    fn error(&self, what: &str) -> io::Error {
        // SAFETY: `self.ssl` is live; the verify error string is a static C string.
        let verify = unsafe { SSL_get_verify_result(self.ssl) };
        let why = if verify != X509_V_OK && matches!(self.check, Check::System | Check::SystemOnline) {
            // SAFETY: as above.
            let s = unsafe { CStr::from_ptr(X509_verify_cert_error_string(verify)) };
            format!("certificate verification failed: {}", s.to_string_lossy())
        } else {
            last_error()
        };
        io::Error::other(format!("{what}: {why}"))
    }

    fn handshake(&mut self) -> io::Result<()> {
        // SAFETY: `self.ssl` is live and owned by this session.
        let r = unsafe {
            ERR_clear_error();
            SSL_do_handshake(self.ssl)
        };
        if r == 1 {
            if let Check::Pinned(pin) = self.check {
                let leaf = self.peer();
                if leaf != Some(pin) {
                    self.drain();
                    return Err(io::Error::other(format!(
                        "TLS handshake failed: the server's certificate is not the accepted one (it presented {})",
                        leaf.map_or_else(|| "none".to_owned(), |p| p.to_string())
                    )));
                }
            }
            // done only once the certificate passed (SEC-07)
            self.handshaking = false;
        } else {
            // SAFETY: as above, with the return value of the call just made.
            let e = unsafe { SSL_get_error(self.ssl, r) };
            if e != SSL_ERROR_WANT_READ && e != SSL_ERROR_WANT_WRITE {
                self.drain();
                return Err(self.error("TLS handshake failed"));
            }
        }
        self.drain();
        Ok(())
    }

    fn peer(&self) -> Option<Fingerprint> {
        self.verification.as_ref().and_then(|v| v.leaf)
    }

    /// Moves ciphertext OpenSSL produced into `out`.
    fn drain(&mut self) {
        let mut buf = [0u8; 16 * 1024];
        loop {
            // SAFETY: `wbio` is live (owned by `ssl`) and `buf` has room for `buf.len()` bytes.
            let n = unsafe { BIO_read(self.wbio, buf.as_mut_ptr().cast(), buf.len() as c_int) };
            if n <= 0 {
                return;
            }
            self.out.extend_from_slice(&buf[..n as usize]);
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
        for chunk in ciphertext.chunks(1 << 20) {
            // SAFETY: `rbio` is a live memory BIO; it copies `chunk`, growing as needed.
            let n = unsafe { BIO_write(self.rbio, chunk.as_ptr().cast(), chunk.len() as c_int) };
            if n != chunk.len() as c_int {
                return Err(io::Error::other("TLS receive failed: out of memory"));
            }
        }
        if self.handshaking {
            self.handshake()?;
        }
        if self.handshaking {
            return Ok(());
        }
        let mut buf = [0u8; 16 * 1024];
        loop {
            // SAFETY: `self.ssl` is live; `buf` has room for `buf.len()` bytes.
            let n = unsafe {
                ERR_clear_error();
                SSL_read(self.ssl, buf.as_mut_ptr().cast(), buf.len() as c_int)
            };
            if n > 0 {
                plain.extend_from_slice(&buf[..n as usize]);
                continue;
            }
            // SAFETY: as above, with the return value of the call just made.
            match unsafe { SSL_get_error(self.ssl, n) } {
                SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => break,
                SSL_ERROR_ZERO_RETURN => {
                    self.closed = true;
                    break;
                }
                _ => return Err(self.error("TLS receive failed")),
            }
        }
        self.drain();
        Ok(())
    }

    fn encrypt(&mut self, plain: &[u8]) -> io::Result<()> {
        debug_assert!(!self.handshaking);
        for chunk in plain.chunks(16 * 1024) {
            // SAFETY: `self.ssl` is live; `chunk` is valid for `chunk.len()` bytes. The memory
            // BIO never blocks, so the whole chunk is written or the call fails.
            let n = unsafe {
                ERR_clear_error();
                SSL_write(self.ssl, chunk.as_ptr().cast(), chunk.len() as c_int)
            };
            if n != chunk.len() as c_int {
                return Err(self.error("TLS send failed"));
            }
        }
        self.drain();
        Ok(())
    }

    fn outgoing(&mut self) -> &mut Vec<u8> {
        &mut self.out
    }

    fn closed(&self) -> bool {
        self.closed
    }

    fn peer_fingerprint(&self) -> Option<Fingerprint> {
        if self.handshaking { None } else { self.peer() }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: `ssl` is live and freed once; this also frees both BIOs. The verification box
        // is dropped after, when nothing can call back into it any more.
        unsafe { SSL_free(self.ssl) };
    }
}

// --- Server identities ---------------------------------------------------------------------

/// A server context with the certificate, its chain and the private key.
pub(crate) struct Identity {
    ctx: Handle,
}

// SAFETY: the context is configured in `new` and afterwards only passed to SSL_new, which
// OpenSSL allows from any thread; it is freed once, in Drop.
unsafe impl Send for Identity {}
// SAFETY: as above.
unsafe impl Sync for Identity {}

impl Identity {
    pub fn new(certs: &[Vec<u8>], key: &pem::Block) -> io::Result<Identity> {
        // SAFETY: a new context we own (freed in Drop, also on the error paths below).
        let id = unsafe {
            ERR_clear_error();
            Identity { ctx: SSL_CTX_new(TLS_server_method()) }
        };
        if id.ctx.is_null() {
            return Err(failed("TLS is not available"));
        }
        // SAFETY: the context is live; the DER buffers are valid for their lengths, and every
        // object d2i returns is freed here or handed to the context (EXTRA_CHAIN_CERT takes
        // ownership on success). A failed minimum-version call (an old LibreSSL) is harmless:
        // their defaults are TLS 1.2 or later anyway.
        unsafe {
            SSL_CTX_ctrl(id.ctx, SSL_CTRL_SET_MIN_PROTO_VERSION, TLS1_2_VERSION, ptr::null_mut());
            for (i, der) in certs.iter().enumerate() {
                let mut p = der.as_ptr();
                let x509 = d2i_X509(ptr::null_mut(), &mut p, der.len() as c_long);
                if x509.is_null() {
                    return Err(failed("the certificate cannot be read"));
                }
                if i == 0 {
                    let ok = SSL_CTX_use_certificate(id.ctx, x509) == 1;
                    X509_free(x509);
                    if !ok {
                        return Err(failed("the certificate cannot be used"));
                    }
                } else if SSL_CTX_ctrl(id.ctx, SSL_CTRL_EXTRA_CHAIN_CERT, 0, x509) != 1 {
                    X509_free(x509);
                    return Err(failed("a certificate of the chain cannot be used"));
                }
            }
            let mut p = key.der.as_ptr();
            let pkey = d2i_AutoPrivateKey(ptr::null_mut(), &mut p, key.der.len() as c_long);
            if pkey.is_null() {
                return Err(failed("the private key cannot be read"));
            }
            let ok = SSL_CTX_use_PrivateKey(id.ctx, pkey) == 1;
            EVP_PKEY_free(pkey);
            if !ok || SSL_CTX_check_private_key(id.ctx) != 1 {
                ERR_clear_error();
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the private key does not belong to the certificate",
                ));
            }
        }
        Ok(id)
    }
}

impl Drop for Identity {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            // SAFETY: created in `new`, freed once; sessions hold their own reference.
            unsafe { SSL_CTX_free(self.ctx) };
        }
    }
}

// --- Self-signed certificates --------------------------------------------------------------

/// Owned OpenSSL objects, freed on drop.
struct Owned(Handle, unsafe extern "C" fn(Handle));

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from the matching constructor and is freed once.
            unsafe { (self.1)(self.0) };
        }
    }
}

unsafe extern "C" fn free_bio(bio: Handle) {
    // SAFETY: the caller passes a BIO it owns.
    unsafe { BIO_free(bio) };
}

/// Everything written to a memory BIO, as text.
fn bio_text(bio: Handle) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        // SAFETY: `bio` is a live memory BIO and `buf` has room for `buf.len()` bytes.
        let n = unsafe { BIO_read(bio, buf.as_mut_ptr().cast(), buf.len() as c_int) };
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// See [`super::generate_self_signed`]: a new P-256 key and an X.509 v3 certificate signed with
/// it, both written as PEM.
pub(crate) fn generate_self_signed(
    cn: &str,
    names: &[SubjectName],
    not_before: i64,
    not_after: i64,
) -> io::Result<(String, String)> {
    let alt: Vec<String> = names
        .iter()
        .map(|n| match n {
            SubjectName::Dns(d) => format!("DNS:{d}"),
            SubjectName::Ip(ip) => format!("IP:{ip}"),
        })
        .collect();
    let alt = CString::new(alt.join(",")).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut serial = [0u8; 8];
    crate::util::random::crypto_random_bytes(&mut serial);
    // positive and non-zero
    let serial = (u64::from_le_bytes(serial) >> 2 | 1) as c_long;
    // SAFETY: every object is created here, owned by an `Owned` guard (or handed to the object
    // that frees it), and only used while alive; strings passed in are NUL-terminated or come
    // with their length.
    unsafe {
        ERR_clear_error();
        let ec = Owned(EC_KEY_new_by_curve_name(NID_X9_62_PRIME256V1), EC_KEY_free);
        if ec.0.is_null() || EC_KEY_generate_key(ec.0) != 1 {
            return Err(failed("cannot create a key"));
        }
        let pkey = Owned(EVP_PKEY_new(), EVP_PKEY_free);
        if pkey.0.is_null() || EVP_PKEY_set1_EC_KEY(pkey.0, ec.0) != 1 {
            return Err(failed("cannot create a key"));
        }
        let x509 = Owned(X509_new(), X509_free);
        if x509.0.is_null() {
            return Err(failed("cannot create the certificate"));
        }
        let x = x509.0;
        let name = X509_get_subject_name(x);
        let mut ok = X509_set_version(x, 2) == 1
            && ASN1_INTEGER_set(X509_get_serialNumber(x), serial) == 1
            && !ASN1_TIME_set(X509_getm_notBefore(x), not_before).is_null()
            && !ASN1_TIME_set(X509_getm_notAfter(x), not_after).is_null()
            && X509_set_pubkey(x, pkey.0) == 1
            && X509_NAME_add_entry_by_txt(
                name,
                c"CN".as_ptr(),
                MBSTRING_UTF8,
                cn.as_ptr(),
                cn.len() as c_int,
                -1,
                0,
            ) == 1
            && X509_set_issuer_name(x, name) == 1;
        for (nid, value) in [(NID_SUBJECT_ALT_NAME, alt.as_c_str()), (NID_EXT_KEY_USAGE, c"serverAuth")] {
            if !ok {
                break;
            }
            let ext = X509V3_EXT_conf_nid(ptr::null_mut(), ptr::null_mut(), nid, value.as_ptr());
            ok = !ext.is_null() && X509_add_ext(x, ext, -1) == 1;
            if !ext.is_null() {
                X509_EXTENSION_free(ext);
            }
        }
        if !ok || X509_sign(x, pkey.0, EVP_sha256()) == 0 {
            return Err(failed("cannot create the certificate"));
        }
        let cert_bio = Owned(BIO_new(BIO_s_mem()), free_bio);
        let key_bio = Owned(BIO_new(BIO_s_mem()), free_bio);
        if cert_bio.0.is_null()
            || key_bio.0.is_null()
            || PEM_write_bio_X509(cert_bio.0, x) != 1
            || PEM_write_bio_PKCS8PrivateKey(
                key_bio.0,
                pkey.0,
                ptr::null(),
                ptr::null(),
                0,
                ptr::null(),
                ptr::null_mut(),
            ) != 1
        {
            return Err(failed("cannot write the certificate"));
        }
        Ok((bio_text(cert_bio.0), bio_text(key_bio.0)))
    }
}
