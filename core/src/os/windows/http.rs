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
        // Missing, or a date instead of seconds: none.
        let mut seconds = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let retry_after = WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_RETRY_AFTER | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut seconds as *mut u32 as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        )
        .is_ok()
        .then_some(seconds);

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
