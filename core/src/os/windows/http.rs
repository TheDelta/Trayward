//! Minimal HTTPS client on WinHTTP, which ships with Windows (no TLS crate).

use std::ffi::c_void;
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_RETRY_AFTER, WINHTTP_QUERY_STATUS_CODE,
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WinHttpSetTimeouts,
};
use windows::core::{HSTRING, PCWSTR, w};

pub use crate::os::Response;

/// Responses larger than this are cut off; the usage API returns a few hundred bytes.
const MAX_BODY: usize = 1 << 20;

struct Handle(*mut c_void);

impl Handle {
    fn new(h: *mut c_void, what: &str) -> Result<Handle, String> {
        if h.is_null() {
            Err(format!("{what} failed ({})", windows::core::Error::from_thread().message()))
        } else {
            Ok(Handle(h))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: non-null WinHTTP handle owned by us, closed once.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn net_err(e: windows::core::Error) -> String {
    format!("Network error: {}", e.message().trim())
}

/// `GET https://{host}{path}` with the given extra request headers.
pub fn get(host: &str, path: &str, headers: &[(&str, &str)]) -> Result<Response, String> {
    request("GET", (host, 443, true), path, headers, &[])
}

/// `POST https://{host}:{port}{path}` with a body; `http://` when `secure` is
/// false (only a push address the user chose, on their own network).
pub fn post(
    (host, port, secure): (&str, u16, bool),
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<Response, String> {
    request("POST", (host, port, secure), path, headers, body)
}

fn request(
    method: &str,
    (host, port, secure): (&str, u16, bool),
    path: &str,
    headers: &[(&str, &str)],
    payload: &[u8],
) -> Result<Response, String> {
    let header_block = headers.iter().fold(String::new(), |block, (k, v)| block + k + ": " + v + "\r\n");
    let header_block: Vec<u16> = header_block.encode_utf16().collect();
    // SAFETY: all handles are checked for null and closed by `Handle`; buffers outlive the calls.
    unsafe {
        let session = Handle::new(
            WinHttpOpen(w!("trayward/0.1"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
            "WinHttpOpen",
        )?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 15_000).map_err(net_err)?;
        let conn = Handle::new(WinHttpConnect(session.0, &HSTRING::from(host), port, 0), "Connect")?;
        let req = Handle::new(
            WinHttpOpenRequest(
                conn.0,
                &HSTRING::from(method),
                &HSTRING::from(path),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                if secure { WINHTTP_FLAG_SECURE } else { Default::default() },
            ),
            "OpenRequest",
        )?;
        // Redirects aren't followed: the request (and its Authorization header)
        // goes to the host it was meant for and nowhere else. A 3xx is an error.
        WinHttpSetOption(Some(req.0), WINHTTP_OPTION_DISABLE_FEATURE, Some(&WINHTTP_DISABLE_REDIRECTS.to_ne_bytes()))
            .map_err(net_err)?;
        let len = payload.len() as u32;
        let optional = (!payload.is_empty()).then_some(payload.as_ptr() as *const c_void);
        WinHttpSendRequest(req.0, Some(&header_block), optional, len, len, 0).map_err(net_err)?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(net_err)?;

        let mut status = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(net_err)?;
        // As text: WinHTTP doesn't hand this header out as a number. Missing, or a
        // date instead of seconds: none.
        let mut text = [0u16; 64];
        let mut size = std::mem::size_of_val(&text) as u32;
        let retry_after = WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_RETRY_AFTER,
            PCWSTR::null(),
            Some(text.as_mut_ptr() as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        )
        .ok()
        .and_then(|()| text.get(..size as usize / 2))
        .and_then(|units| String::from_utf16_lossy(units).trim().parse::<u32>().ok());

        let mut body = Vec::new();
        loop {
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(req.0, &mut avail).map_err(net_err)?;
            if avail == 0 || body.len() >= MAX_BODY {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read = 0u32;
            let Some(room) = body.get_mut(start..) else { break };
            WinHttpReadData(req.0, room.as_mut_ptr() as *mut c_void, avail, &mut read).map_err(net_err)?;
            body.truncate(start + read as usize);
            if read == 0 {
                break;
            }
        }
        Ok(Response { status, body, retry_after })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    /// A server on 127.0.0.1 that answers one request with `answer` and gives back
    /// what it received. `read` says whether it waits for the whole request first.
    fn serve(answer: &'static str, read: bool) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            if read {
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    got.extend_from_slice(buf.get(..n).unwrap_or_default());
                    let text = String::from_utf8_lossy(&got).into_owned();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else {
                        if n == 0 {
                            break;
                        }
                        continue;
                    };
                    let length = head
                        .lines()
                        .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")))
                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if n == 0 || body.len() >= length {
                        break;
                    }
                }
            }
            s.write_all(answer.as_bytes()).unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });
        (port, server)
    }

    fn post_to(port: u16, secure: bool) -> Result<Response, String> {
        post(("127.0.0.1", port, secure), "/push?to=me", &[("X-Test", "yes"), ("Content-Type", "text/plain")], b"hello")
    }

    #[test]
    fn sends_the_request_and_reads_the_answer() {
        let (port, server) = serve("HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nthanks!", true);
        let r = post_to(port, false).unwrap();
        assert_eq!((r.status, r.body.as_slice(), r.retry_after), (200, &b"thanks!"[..], None));
        let got = server.join().unwrap();
        assert!(got.starts_with("POST /push?to=me HTTP/1.1\r\n"), "{got}");
        assert!(got.contains("X-Test: yes\r\n") && got.contains("Content-Length: 5\r\n"), "{got}");
        assert!(got.ends_with("\r\n\r\nhello"), "{got}");
    }

    #[test]
    fn reads_how_long_to_wait() {
        let (port, server) =
            serve("HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\nContent-Length: 0\r\n\r\n", true);
        let r = post_to(port, false).unwrap();
        assert_eq!((r.status, r.retry_after), (429, Some(30)));
        server.join().unwrap();
        // A date instead of seconds: none.
        let date = "HTTP/1.1 503 Busy\r\nRetry-After: Wed, 21 Oct 2026 07:28:00 GMT\r\nContent-Length: 0\r\n\r\n";
        let (port, server) = serve(date, true);
        let r = post_to(port, false).unwrap();
        assert_eq!((r.status, r.retry_after), (503, None));
        server.join().unwrap();
    }

    /// The request and its headers go to the host they were meant for and nowhere else.
    #[test]
    fn follows_no_redirect() {
        let (port, server) =
            serve("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/elsewhere\r\nContent-Length: 0\r\n\r\n", true);
        assert_eq!(post_to(port, false).unwrap().status, 302);
        server.join().unwrap();
    }

    #[test]
    fn says_why_it_failed() {
        // Nobody listens there.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(post_to(port, false).err().is_some_and(|e| e.starts_with("Network error")));
        // HTTPS to a server that only speaks plain HTTP.
        let (port, server) = serve("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", false);
        assert!(post_to(port, true).err().is_some_and(|e| e.starts_with("Network error")));
        server.join().unwrap();
    }
}
