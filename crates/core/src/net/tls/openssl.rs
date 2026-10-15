// PaTorrent's own OS glue, not ported (libtorrent uses OpenSSL through Boost.Asio).

//! TLS client sessions on the system's OpenSSL (1.1.1 or later) or LibreSSL.
//!
//! The session talks to OpenSSL through two memory BIOs, so it never touches the socket. Peer
//! verification is on (`SSL_VERIFY_PEER` against the default trust store), the certificate must
//! match the host name (`SSL_set1_host`) or IP address, and SNI is sent for host names. Only
//! functions present in OpenSSL 1.1.1, 3.x and LibreSSL are used, declared here by hand so no
//! binding crate or build script is needed; linking needs `libssl` and `libcrypto`.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_ulong, c_void};
use std::io;
use std::net::IpAddr;
use std::ptr;
use std::sync::OnceLock;

type Handle = *mut c_void;

const SSL_VERIFY_PEER: c_int = 1;
const SSL_ERROR_WANT_READ: c_int = 2;
const SSL_ERROR_WANT_WRITE: c_int = 3;
const SSL_ERROR_ZERO_RETURN: c_int = 6;
const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
const TLSEXT_NAMETYPE_HOST_NAME: c_long = 0;
const X509_V_OK: c_long = 0;

#[link(name = "ssl")]
#[link(name = "crypto")]
unsafe extern "C" {
    fn TLS_client_method() -> *const c_void;
    fn SSL_CTX_new(method: *const c_void) -> Handle;
    fn SSL_CTX_set_default_verify_paths(ctx: Handle) -> c_int;
    fn SSL_CTX_set_verify(ctx: Handle, mode: c_int, callback: *const c_void);
    fn SSL_new(ctx: Handle) -> Handle;
    fn SSL_free(ssl: Handle);
    fn SSL_set_bio(ssl: Handle, rbio: Handle, wbio: Handle);
    fn SSL_set_connect_state(ssl: Handle);
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
    fn ERR_get_error() -> c_ulong;
    fn ERR_error_string_n(e: c_ulong, buf: *mut c_char, len: usize);
    fn ERR_clear_error();
    fn BIO_new(method: *const c_void) -> Handle;
    fn BIO_s_mem() -> *const c_void;
    fn BIO_free(bio: Handle) -> c_int;
    fn BIO_read(bio: Handle, data: *mut c_void, len: c_int) -> c_int;
    fn BIO_write(bio: Handle, data: *const c_void, len: c_int) -> c_int;
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

pub(crate) struct Session {
    ssl: Handle,
    /// Network → OpenSSL; owned by `ssl`.
    rbio: Handle,
    /// OpenSSL → network; owned by `ssl`.
    wbio: Handle,
    out: Vec<u8>,
    handshaking: bool,
    closed: bool,
}

// SAFETY: the SSL object and its BIOs belong to this session alone and are only used through
// `&mut self`, so moving the session to another thread is fine.
unsafe impl Send for Session {}

impl Session {
    pub fn new(host: &str) -> io::Result<Session> {
        let name = CString::new(host)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid host name"))?;
        let ctx = context()?;
        // SAFETY: `ctx` is the live process context. The session frees the SSL object on drop;
        // once SSL_set_bio has run, the SSL object owns both BIOs and frees them with itself.
        let mut s = unsafe {
            ERR_clear_error();
            let ssl = SSL_new(ctx);
            if ssl.is_null() {
                return Err(io::Error::other(format!("TLS setup failed: {}", last_error())));
            }
            let mut s = Session {
                ssl,
                rbio: ptr::null_mut(),
                wbio: ptr::null_mut(),
                out: Vec::new(),
                handshaking: true,
                closed: false,
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
            SSL_set_connect_state(ssl);
            (s.rbio, s.wbio) = (rbio, wbio);
            s
        };
        // SAFETY: `s.ssl` is live and `name` is NUL-terminated; OpenSSL copies the name.
        let ok = unsafe {
            if host.parse::<IpAddr>().is_ok() {
                X509_VERIFY_PARAM_set1_ip_asc(SSL_get0_param(s.ssl), name.as_ptr()) == 1
            } else {
                SSL_ctrl(
                    s.ssl,
                    SSL_CTRL_SET_TLSEXT_HOSTNAME,
                    TLSEXT_NAMETYPE_HOST_NAME,
                    name.as_ptr() as *mut c_void,
                ) == 1
                    && SSL_set1_host(s.ssl, name.as_ptr()) == 1
            }
        };
        if !ok {
            return Err(io::Error::other(format!("TLS setup failed: {}", last_error())));
        }
        s.handshake()?;
        Ok(s)
    }

    fn error(&self, what: &str) -> io::Error {
        // SAFETY: `self.ssl` is live; the verify error string is a static C string.
        let verify = unsafe { SSL_get_verify_result(self.ssl) };
        let why = if verify != X509_V_OK {
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
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: `ssl` is live and freed once; this also frees both BIOs.
        unsafe { SSL_free(self.ssl) };
    }
}
