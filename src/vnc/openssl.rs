//! Anonymous TLS, through OpenSSL loaded at run time. GNOME Remote
//! Desktop's VNC (security type 18) and TigerVNC's VeNCrypt TLS subtypes
//! encrypt with Diffie-Hellman and no certificate, which rustls doesn't
//! implement. Nothing links OpenSSL: a computer without it can still use
//! every other kind of security. OpenSSL reads and writes memory buffers
//! here, and the stream around it moves their bytes to and from the
//! connection, so tokio drives the socket as it does for everything else.

use std::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};
use std::io;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll, ready};

use libloading::Library;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::remote::route::Stream;

const SSL_ERROR_WANT_READ: c_int = 2;
const SSL_ERROR_ZERO_RETURN: c_int = 6;
const SSL_CTRL_SET_MAX_PROTO_VERSION: c_int = 124;
const TLS1_2_VERSION: c_long = 0x0303;
const BIO_CTRL_PENDING: c_int = 10;

/// The anonymous suites, Diffie-Hellman with and without elliptic curves,
/// and none without encryption. They sit below every security level but
/// 0, and exist in TLS 1.2 alone.
const CIPHERS: &CStr = c"aNULL:!eNULL:@SECLEVEL=0";

type Ptr = *mut c_void;

struct Api {
    client_method: unsafe extern "C" fn() -> *const c_void,
    ctx_new: unsafe extern "C" fn(*const c_void) -> Ptr,
    ctx_free: unsafe extern "C" fn(Ptr),
    ctx_set_cipher_list: unsafe extern "C" fn(Ptr, *const c_char) -> c_int,
    ctx_ctrl: unsafe extern "C" fn(Ptr, c_int, c_long, Ptr) -> c_long,
    new: unsafe extern "C" fn(Ptr) -> Ptr,
    free: unsafe extern "C" fn(Ptr),
    set_bio: unsafe extern "C" fn(Ptr, Ptr, Ptr),
    set_connect_state: unsafe extern "C" fn(Ptr),
    do_handshake: unsafe extern "C" fn(Ptr) -> c_int,
    read: unsafe extern "C" fn(Ptr, Ptr, c_int) -> c_int,
    write: unsafe extern "C" fn(Ptr, *const c_void, c_int) -> c_int,
    get_error: unsafe extern "C" fn(Ptr, c_int) -> c_int,
    current_cipher: unsafe extern "C" fn(Ptr) -> *const c_void,
    cipher_name: unsafe extern "C" fn(*const c_void) -> *const c_char,
    bio_s_mem: unsafe extern "C" fn() -> *const c_void,
    bio_new: unsafe extern "C" fn(*const c_void) -> Ptr,
    bio_read: unsafe extern "C" fn(Ptr, Ptr, c_int) -> c_int,
    bio_write: unsafe extern "C" fn(Ptr, *const c_void, c_int) -> c_int,
    bio_ctrl: unsafe extern "C" fn(Ptr, c_int, c_long, Ptr) -> c_long,
    err_get: unsafe extern "C" fn() -> c_ulong,
    err_string: unsafe extern "C" fn(c_ulong, *mut c_char, usize),
    // Kept loaded for as long as the functions above are used.
    _ssl: Library,
    _crypto: Library,
}

impl Api {
    /// SAFETY: the libraries must be OpenSSL 3's.
    unsafe fn load() -> Result<Self, libloading::Error> {
        unsafe {
            let crypto = Library::new("libcrypto.so.3")?;
            let ssl = Library::new("libssl.so.3")?;
            Ok(Self {
                client_method: *ssl.get(b"TLS_client_method\0")?,
                ctx_new: *ssl.get(b"SSL_CTX_new\0")?,
                ctx_free: *ssl.get(b"SSL_CTX_free\0")?,
                ctx_set_cipher_list: *ssl.get(b"SSL_CTX_set_cipher_list\0")?,
                ctx_ctrl: *ssl.get(b"SSL_CTX_ctrl\0")?,
                new: *ssl.get(b"SSL_new\0")?,
                free: *ssl.get(b"SSL_free\0")?,
                set_bio: *ssl.get(b"SSL_set_bio\0")?,
                set_connect_state: *ssl.get(b"SSL_set_connect_state\0")?,
                do_handshake: *ssl.get(b"SSL_do_handshake\0")?,
                read: *ssl.get(b"SSL_read\0")?,
                write: *ssl.get(b"SSL_write\0")?,
                get_error: *ssl.get(b"SSL_get_error\0")?,
                current_cipher: *ssl.get(b"SSL_get_current_cipher\0")?,
                cipher_name: *ssl.get(b"SSL_CIPHER_get_name\0")?,
                bio_s_mem: *crypto.get(b"BIO_s_mem\0")?,
                bio_new: *crypto.get(b"BIO_new\0")?,
                bio_read: *crypto.get(b"BIO_read\0")?,
                bio_write: *crypto.get(b"BIO_write\0")?,
                bio_ctrl: *crypto.get(b"BIO_ctrl\0")?,
                err_get: *crypto.get(b"ERR_get_error\0")?,
                err_string: *crypto.get(b"ERR_error_string_n\0")?,
                _ssl: ssl,
                _crypto: crypto,
            })
        }
    }

    /// OpenSSL's queued errors for this thread, as one line.
    fn errors(&self) -> String {
        let mut lines = Vec::new();
        loop {
            // SAFETY: reads and clears this thread's error queue.
            let code = unsafe { (self.err_get)() };
            if code == 0 {
                break;
            }
            let mut text = [0 as c_char; 256];
            // SAFETY: writes at most the buffer's length, NUL included.
            unsafe { (self.err_string)(code, text.as_mut_ptr(), text.len()) };
            // SAFETY: NUL-terminated by OpenSSL.
            lines.push(
                unsafe { CStr::from_ptr(text.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if lines.is_empty() {
            "OpenSSL gave no reason".into()
        } else {
            lines.join("; ")
        }
    }
}

/// OpenSSL, if this computer has it.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: these sonames are OpenSSL 3's.
        match unsafe { Api::load() } {
            Ok(api) => Some(api),
            Err(e) => {
                tracing::info!("no OpenSSL for anonymous TLS: {e}");
                None
            }
        }
    })
    .as_ref()
}

pub(super) fn available() -> bool {
    api().is_some()
}

/// Why anonymous TLS couldn't start.
pub(super) enum Error {
    /// This computer has no OpenSSL 3.
    Missing,
    Io(io::Error),
    /// The handshake failed, as OpenSSL says.
    Tls(String),
}

/// A connection, encrypted with anonymous TLS.
pub(super) struct AnonTls {
    api: &'static Api,
    inner: Box<dyn Stream>,
    /// The connection, which owns both memory buffers.
    ssl: Ptr,
    /// Bytes from the server, for OpenSSL to read.
    from_server: Ptr,
    /// Bytes OpenSSL wrote, for the server.
    to_server: Ptr,
    /// Bytes OpenSSL wrote that the connection hasn't taken yet.
    outgoing: Vec<u8>,
}

// SAFETY: the connection is used by one caller at a time, through `&mut`,
// and OpenSSL's objects may move between threads.
unsafe impl Send for AnonTls {}
// SAFETY: nothing is reachable through `&AnonTls`.
unsafe impl Sync for AnonTls {}

impl Drop for AnonTls {
    fn drop(&mut self) {
        // SAFETY: owned here; frees both memory buffers with it.
        unsafe { (self.api.free)(self.ssl) };
    }
}

/// Encrypt `inner` with anonymous TLS, as its client.
pub(super) async fn connect(inner: Box<dyn Stream>) -> Result<AnonTls, Error> {
    let api = api().ok_or(Error::Missing)?;
    // SAFETY: each pointer is checked before use, and every object made
    // here belongs to the next one or to `AnonTls`, which frees it.
    let mut tls = unsafe {
        let ctx = (api.ctx_new)((api.client_method)());
        if ctx.is_null() {
            return Err(Error::Tls(api.errors()));
        }
        let set = (api.ctx_set_cipher_list)(ctx, CIPHERS.as_ptr()) == 1
            && (api.ctx_ctrl)(
                ctx,
                SSL_CTRL_SET_MAX_PROTO_VERSION,
                TLS1_2_VERSION,
                std::ptr::null_mut(),
            ) == 1;
        let ssl = if set {
            (api.new)(ctx)
        } else {
            std::ptr::null_mut()
        };
        // The connection keeps the context it needs.
        (api.ctx_free)(ctx);
        if ssl.is_null() {
            return Err(Error::Tls(api.errors()));
        }
        let from_server = (api.bio_new)((api.bio_s_mem)());
        let to_server = (api.bio_new)((api.bio_s_mem)());
        (api.set_bio)(ssl, from_server, to_server);
        (api.set_connect_state)(ssl);
        AnonTls {
            api,
            inner,
            ssl,
            from_server,
            to_server,
            outgoing: Vec::new(),
        }
    };
    let mut buffer = vec![0; 16 << 10];
    loop {
        // SAFETY: the connection is set up above.
        let result = unsafe { (api.do_handshake)(tls.ssl) };
        tls.take_output();
        let outgoing = std::mem::take(&mut tls.outgoing);
        tls.inner.write_all(&outgoing).await.map_err(Error::Io)?;
        tls.inner.flush().await.map_err(Error::Io)?;
        if result == 1 {
            break;
        }
        // SAFETY: as above.
        if unsafe { (api.get_error)(tls.ssl, result) } != SSL_ERROR_WANT_READ {
            return Err(Error::Tls(api.errors()));
        }
        let read = tls.inner.read(&mut buffer).await.map_err(Error::Io)?;
        if read == 0 {
            return Err(Error::Io(io::ErrorKind::UnexpectedEof.into()));
        }
        tls.give_input(&buffer[..read]);
    }
    tracing::info!("anonymous TLS with {}", tls.cipher());
    Ok(tls)
}

impl AnonTls {
    fn cipher(&self) -> String {
        // SAFETY: the connection is up; the name is a static string.
        unsafe {
            let cipher = (self.api.current_cipher)(self.ssl);
            if cipher.is_null() {
                return "no cipher".into();
            }
            CStr::from_ptr((self.api.cipher_name)(cipher))
                .to_string_lossy()
                .into_owned()
        }
    }

    /// Hand OpenSSL bytes from the server.
    fn give_input(&mut self, bytes: &[u8]) {
        // A memory buffer takes everything it is given.
        // SAFETY: `bytes` is valid for its length.
        unsafe {
            (self.api.bio_write)(
                self.from_server,
                bytes.as_ptr().cast(),
                bytes.len() as c_int,
            )
        };
    }

    /// Move what OpenSSL wrote to the queue for the server.
    fn take_output(&mut self) {
        loop {
            // SAFETY: asks how much the memory buffer holds.
            let pending = unsafe {
                (self.api.bio_ctrl)(self.to_server, BIO_CTRL_PENDING, 0, std::ptr::null_mut())
            };
            if pending <= 0 {
                return;
            }
            let start = self.outgoing.len();
            self.outgoing.resize(start + pending as usize, 0);
            // SAFETY: room for `pending` bytes was just made.
            let read = unsafe {
                (self.api.bio_read)(
                    self.to_server,
                    self.outgoing[start..].as_mut_ptr().cast(),
                    pending as c_int,
                )
            };
            self.outgoing.truncate(start + read.max(0) as usize);
        }
    }

    /// Send what is queued for the server.
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while !self.outgoing.is_empty() {
            let written = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.outgoing))?;
            if written == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.outgoing.drain(..written);
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for AnonTls {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let room = buf.remaining().min(c_int::MAX as usize) as c_int;
            // SAFETY: OpenSSL writes at most `room` bytes into the unfilled
            // part of the buffer, which are then marked filled.
            let read = unsafe {
                let unfilled = buf.unfilled_mut();
                (this.api.read)(this.ssl, unfilled.as_mut_ptr().cast(), room)
            };
            if read > 0 {
                // SAFETY: OpenSSL initialized these bytes.
                unsafe { buf.assume_init(read as usize) };
                buf.advance(read as usize);
                return Poll::Ready(Ok(()));
            }
            // SAFETY: the connection is up.
            match unsafe { (this.api.get_error)(this.ssl, read) } {
                SSL_ERROR_WANT_READ => {
                    // Anything OpenSSL wants to say first goes out first.
                    this.take_output();
                    if let Poll::Ready(Err(e)) = this.poll_drain(cx) {
                        return Poll::Ready(Err(e));
                    }
                    let mut bytes = [0; 16 << 10];
                    let mut from = ReadBuf::new(&mut bytes);
                    ready!(Pin::new(&mut this.inner).poll_read(cx, &mut from))?;
                    if from.filled().is_empty() {
                        // The server went away: an end, as far as reading goes.
                        return Poll::Ready(Ok(()));
                    }
                    let filled = from.filled().to_vec();
                    this.give_input(&filled);
                }
                SSL_ERROR_ZERO_RETURN => return Poll::Ready(Ok(())),
                _ => return Poll::Ready(Err(io::Error::other(this.api.errors()))),
            }
        }
    }
}

impl AsyncWrite for AnonTls {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        let length = data.len().min(c_int::MAX as usize) as c_int;
        // SAFETY: `data` is valid for `length` bytes. A memory buffer never
        // fills up, so OpenSSL takes all of them or fails.
        let written = unsafe { (this.api.write)(this.ssl, data.as_ptr().cast(), length) };
        if written <= 0 {
            return Poll::Ready(Err(io::Error::other(this.api.errors())));
        }
        this.take_output();
        // Sent now if the connection takes it, else on the next write or
        // flush.
        if let Poll::Ready(Err(e)) = this.poll_drain(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(written as usize))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
