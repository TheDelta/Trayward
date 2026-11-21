//! TLS over a TCP connection with Windows' own SChannel (no TLS crate), for
//! `mqtts://`. TLS 1.2; the server's certificate is checked by Windows: its
//! chain against the certificate store, its name against the host.

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use windows::Win32::Foundation::{
    SEC_E_INCOMPLETE_MESSAGE, SEC_E_OK, SEC_I_CONTEXT_EXPIRED, SEC_I_CONTINUE_NEEDED, SEC_I_INCOMPLETE_CREDENTIALS,
    SEC_I_RENEGOTIATE,
};
use windows::Win32::Security::Authentication::Identity::{
    AcquireCredentialsHandleW, DecryptMessage, DeleteSecurityContext, EncryptMessage, FreeContextBuffer,
    FreeCredentialsHandle, ISC_REQ_ALLOCATE_MEMORY, ISC_REQ_CONFIDENTIALITY, ISC_REQ_EXTENDED_ERROR,
    ISC_REQ_REPLAY_DETECT, ISC_REQ_SEQUENCE_DETECT, ISC_REQ_STREAM, InitializeSecurityContextW,
    QueryContextAttributesW, SCH_CRED_AUTO_CRED_VALIDATION, SCH_CRED_NO_DEFAULT_CREDS, SCH_USE_STRONG_CRYPTO,
    SCHANNEL_CRED, SCHANNEL_CRED_VERSION, SECBUFFER_DATA, SECBUFFER_EMPTY, SECBUFFER_EXTRA, SECBUFFER_STREAM_HEADER,
    SECBUFFER_STREAM_TRAILER, SECBUFFER_TOKEN, SECBUFFER_VERSION, SECPKG_ATTR_STREAM_SIZES, SECPKG_CRED_OUTBOUND,
    SP_PROT_TLS1_2_CLIENT, SecBuffer, SecBufferDesc, SecPkgContext_StreamSizes, UNISP_NAME_W,
};
use windows::Win32::Security::Credentials::SecHandle;
use windows::core::{HRESULT, HSTRING};

/// A TLS connection. Reads and writes plain bytes; what goes over the wire is encrypted.
pub struct TlsStream {
    tcp: TcpStream,
    cred: SecHandle,
    ctx: SecHandle,
    sizes: SecPkgContext_StreamSizes,
    /// Received from the server, not yet decrypted.
    incoming: Vec<u8>,
    /// Decrypted, not yet read.
    plain: Vec<u8>,
    plain_at: usize,
    /// The server closed the TLS session.
    closed: bool,
}

// SAFETY: the handles belong to this connection alone and are only used through `&mut self`.
unsafe impl Send for TlsStream {}

fn err(status: HRESULT) -> io::Error {
    io::Error::other(windows::core::Error::from(status).message())
}

fn buffer(kind: u32, data: *mut u8, len: usize) -> SecBuffer {
    SecBuffer { cbBuffer: len as u32, BufferType: kind, pvBuffer: data as *mut c_void }
}

impl TlsStream {
    /// Makes the TLS handshake with `host` over `tcp` and checks its certificate.
    pub fn connect(mut tcp: TcpStream, host: &str) -> io::Result<TlsStream> {
        // First: once the credentials exist, only the stream frees them.
        let reader = tcp.try_clone()?;
        let mut cred = SecHandle::default();
        let auth = SCHANNEL_CRED {
            dwVersion: SCHANNEL_CRED_VERSION,
            grbitEnabledProtocols: SP_PROT_TLS1_2_CLIENT,
            dwFlags: SCH_CRED_NO_DEFAULT_CREDS | SCH_CRED_AUTO_CRED_VALIDATION | SCH_USE_STRONG_CRYPTO,
            ..Default::default()
        };
        // SAFETY: `auth` and `cred` outlive the call.
        unsafe {
            AcquireCredentialsHandleW(
                None,
                UNISP_NAME_W,
                SECPKG_CRED_OUTBOUND,
                None,
                Some(&auth as *const _ as *const c_void),
                None,
                None,
                &mut cred,
                None,
            )
        }
        .map_err(io::Error::other)?;
        let mut tls = TlsStream {
            tcp: reader,
            cred,
            ctx: SecHandle::default(),
            sizes: SecPkgContext_StreamSizes::default(),
            incoming: Vec::new(),
            plain: Vec::new(),
            plain_at: 0,
            closed: false,
        };
        let target = HSTRING::from(host);
        let flags = ISC_REQ_SEQUENCE_DETECT
            | ISC_REQ_REPLAY_DETECT
            | ISC_REQ_CONFIDENTIALITY
            | ISC_REQ_EXTENDED_ERROR
            | ISC_REQ_ALLOCATE_MEMORY
            | ISC_REQ_STREAM;
        let mut first = true;
        let mut need_more = false;
        let mut asked_for_certificate = false;
        loop {
            if need_more {
                let mut chunk = [0u8; 16 * 1024];
                let n = tcp.read(&mut chunk)?;
                if n == 0 {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                tls.incoming.extend_from_slice(chunk.get(..n).unwrap_or_default());
            }
            let mut out = [buffer(SECBUFFER_TOKEN, std::ptr::null_mut(), 0)];
            let mut out_desc = SecBufferDesc { ulVersion: SECBUFFER_VERSION, cBuffers: 1, pBuffers: out.as_mut_ptr() };
            let len = tls.incoming.len();
            let mut input = [
                buffer(SECBUFFER_TOKEN, tls.incoming.as_mut_ptr(), len),
                buffer(SECBUFFER_EMPTY, std::ptr::null_mut(), 0),
            ];
            let in_desc = SecBufferDesc { ulVersion: SECBUFFER_VERSION, cBuffers: 2, pBuffers: input.as_mut_ptr() };
            let mut attrs = 0u32;
            let ctx: *mut SecHandle = &mut tls.ctx;
            // SAFETY: the buffers point into `incoming` (or are filled by
            // SChannel, ISC_REQ_ALLOCATE_MEMORY) and outlive the call; the
            // context is read and written in place, as the API intends.
            let status = unsafe {
                InitializeSecurityContextW(
                    Some(&tls.cred),
                    (!first).then_some(ctx as *const SecHandle),
                    Some(target.as_ptr()),
                    flags,
                    0,
                    0,
                    (!first).then_some(&in_desc as *const SecBufferDesc),
                    0,
                    Some(ctx),
                    Some(&mut out_desc),
                    &mut attrs,
                    None,
                )
            };
            if !out[0].pvBuffer.is_null() {
                // SAFETY: SChannel allocated this token; it's sent, then freed.
                let token =
                    unsafe { std::slice::from_raw_parts(out[0].pvBuffer as *const u8, out[0].cbBuffer as usize) };
                let sent = tcp.write_all(token);
                // SAFETY: allocated by SChannel for us.
                let _ = unsafe { FreeContextBuffer(out[0].pvBuffer) };
                sent?;
            }
            if status == SEC_E_INCOMPLETE_MESSAGE {
                need_more = true;
                continue;
            }
            // The server would take a client certificate; there is none.
            // Asked again with the same input, SChannel goes on without one.
            if status == SEC_I_INCOMPLETE_CREDENTIALS && !asked_for_certificate {
                asked_for_certificate = true;
                need_more = false;
                continue;
            }
            if status != SEC_E_OK && status != SEC_I_CONTINUE_NEEDED {
                return Err(err(status));
            }
            first = false;
            // What SChannel didn't use yet stays for the next round.
            let extra = if input[1].BufferType == SECBUFFER_EXTRA { input[1].cbBuffer as usize } else { 0 };
            let used = tls.incoming.len() - extra.min(tls.incoming.len());
            tls.incoming.drain(..used);
            if status == SEC_E_OK {
                break;
            }
            need_more = tls.incoming.is_empty();
        }
        // SAFETY: the context is established; `sizes` is the documented struct for this attribute.
        unsafe {
            QueryContextAttributesW(
                &tls.ctx,
                SECPKG_ATTR_STREAM_SIZES,
                &mut tls.sizes as *mut SecPkgContext_StreamSizes as *mut c_void,
            )
        }
        .map_err(io::Error::other)?;
        Ok(tls)
    }

    /// Decrypts what has come in, if it's a whole record. Ok(false): more is needed.
    fn decrypt(&mut self) -> io::Result<bool> {
        let len = self.incoming.len();
        let none = std::ptr::null_mut();
        let mut bufs = [
            buffer(SECBUFFER_DATA, self.incoming.as_mut_ptr(), len),
            buffer(SECBUFFER_EMPTY, none, 0),
            buffer(SECBUFFER_EMPTY, none, 0),
            buffer(SECBUFFER_EMPTY, none, 0),
        ];
        let desc = SecBufferDesc { ulVersion: SECBUFFER_VERSION, cBuffers: 4, pBuffers: bufs.as_mut_ptr() };
        // SAFETY: decrypts in place inside `incoming`, which outlives the call.
        let status = unsafe { DecryptMessage(&self.ctx, &desc, 0, None) };
        if status == SEC_E_INCOMPLETE_MESSAGE {
            return Ok(false);
        }
        if status == SEC_I_CONTEXT_EXPIRED {
            self.closed = true;
            return Ok(true);
        }
        if status == SEC_I_RENEGOTIATE {
            return Err(io::Error::other("the server wants to renegotiate TLS"));
        }
        if status != SEC_E_OK {
            return Err(err(status));
        }
        self.plain.clear();
        self.plain_at = 0;
        let mut extra = 0;
        for b in &bufs {
            if b.BufferType == SECBUFFER_DATA && !b.pvBuffer.is_null() {
                // SAFETY: SChannel points this into `incoming`, `cbBuffer` long.
                self.plain.extend_from_slice(unsafe {
                    std::slice::from_raw_parts(b.pvBuffer as *const u8, b.cbBuffer as usize)
                });
            } else if b.BufferType == SECBUFFER_EXTRA {
                extra = b.cbBuffer as usize;
            }
        }
        // The next record's start (if any) is at the end.
        let used = self.incoming.len() - extra.min(self.incoming.len());
        self.incoming.drain(..used);
        Ok(true)
    }
}

impl Read for TlsStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.plain_at < self.plain.len() {
                let n = out.len().min(self.plain.len() - self.plain_at);
                let plain = self.plain.iter().skip(self.plain_at);
                for (to, from) in out.iter_mut().zip(plain) {
                    *to = *from;
                }
                self.plain_at += n;
                return Ok(n);
            }
            if self.closed {
                return Ok(0);
            }
            if !self.incoming.is_empty() && self.decrypt()? {
                continue;
            }
            let mut chunk = [0u8; 16 * 1024];
            let n = self.tcp.read(&mut chunk)?;
            if n == 0 {
                return Ok(0);
            }
            self.incoming.extend_from_slice(chunk.get(..n).unwrap_or_default());
        }
    }
}

impl Write for TlsStream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let s = self.sizes;
        let n = data.len().min(s.cbMaximumMessage as usize);
        let (head, tail) = (s.cbHeader as usize, s.cbTrailer as usize);
        let mut msg = vec![0u8; head + n + tail];
        for (to, from) in msg.iter_mut().skip(head).zip(data.iter().take(n)) {
            *to = *from;
        }
        let p = msg.as_mut_ptr();
        // SAFETY: the three buffers are consecutive parts of `msg`.
        let mut bufs = unsafe {
            [
                buffer(SECBUFFER_STREAM_HEADER, p, head),
                buffer(SECBUFFER_DATA, p.add(head), n),
                buffer(SECBUFFER_STREAM_TRAILER, p.add(head + n), tail),
                buffer(SECBUFFER_EMPTY, std::ptr::null_mut(), 0),
            ]
        };
        let desc = SecBufferDesc { ulVersion: SECBUFFER_VERSION, cBuffers: 4, pBuffers: bufs.as_mut_ptr() };
        // SAFETY: encrypts in place inside `msg`, which outlives the call.
        let status = unsafe { EncryptMessage(&self.ctx, 0, &desc, 0) };
        if status != SEC_E_OK {
            return Err(err(status));
        }
        let total = (bufs[0].cbBuffer + bufs[1].cbBuffer + bufs[2].cbBuffer) as usize;
        self.tcp.write_all(msg.get(..total).unwrap_or(&msg))?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

impl Drop for TlsStream {
    fn drop(&mut self) {
        // SAFETY: handles acquired in `connect`, released once.
        unsafe {
            let _ = DeleteSecurityContext(&self.ctx);
            let _ = FreeCredentialsHandle(&self.cred);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A server on 127.0.0.1 that sends `answer` to the first connection, then hangs up.
    fn serve(answer: &'static [u8]) -> (TcpStream, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.write_all(answer);
        });
        let tcp = TcpStream::connect(addr).unwrap();
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        (tcp, server)
    }

    /// A server that hangs up, or answers in plain text instead of TLS: no connection.
    #[test]
    fn refuses_anything_but_tls() {
        let (tcp, server) = serve(b"");
        server.join().unwrap();
        assert!(TlsStream::connect(tcp, "localhost").is_err());
        let (tcp, server) = serve(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
        let refused = TlsStream::connect(tcp, "localhost");
        server.join().unwrap();
        assert!(refused.is_err());
    }

    /// A real TLS connection, to the host the update check talks to anyway:
    /// the handshake, the certificate check, and a request and its answer.
    #[test]
    #[ignore = "needs the network"]
    fn talks_tls() {
        let tcp = TcpStream::connect(("api.github.com", 443)).unwrap();
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
        let mut tls = TlsStream::connect(tcp, "api.github.com").unwrap();
        tls.write_all(
            b"GET /zen HTTP/1.1\r\nHost: api.github.com\r\nUser-Agent: trayward-test\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        let mut answer = Vec::new();
        let _ = tls.read_to_end(&mut answer);
        let text = String::from_utf8_lossy(&answer);
        assert!(text.starts_with("HTTP/1.1 200"), "{}", text.chars().take(200).collect::<String>());
        // A name that doesn't match the certificate is refused.
        let tcp = TcpStream::connect(("api.github.com", 443)).unwrap();
        assert!(TlsStream::connect(tcp, "not-github.example").is_err());
    }
}
