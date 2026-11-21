//! Any system but Windows, for now: plain fallbacks so the logic builds and its
//! tests run there. A real port replaces these with the system's own APIs
//! (see ROADMAP.md, "Other operating systems").
//!
//! - Processes: Linux's `/proc` tells whether one runs; nothing else is known.
//! - Time: UTC, in English.
//! - HTTPS and TLS: not available.

use super::ProcEntry;
use std::collections::HashMap;
use std::path::PathBuf;

/// When the process started: unknown here.
pub fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

/// Whether the process `pid` runs. Unknown counts as yes, as on Windows for a
/// process that may not be inspected.
pub fn process_alive(pid: u32, _started: Option<u64>) -> bool {
    if crate::demo::active() && pid >= crate::demo::PID_BASE {
        return true;
    }
    let proc = std::path::Path::new("/proc");
    !proc.is_dir() || proc.join(pid.to_string()).exists()
}

/// Whether the process `pid` started no later than `unix_secs`: unknown, so yes.
pub fn started_by(_pid: u32, _unix_secs: i64) -> bool {
    true
}

/// The executable of a running process: unknown here.
pub fn process_image(_pid: u32) -> Option<PathBuf> {
    None
}

/// All processes: none known here.
pub fn process_table() -> HashMap<u32, ProcEntry> {
    HashMap::new()
}

pub fn trim_heap() {}

/// Changes in a folder: not told here, so whoever waits looks again after the time.
pub struct DirWatch;

impl DirWatch {
    /// Never available here: the caller looks at the folder now and then instead.
    pub fn new(_dir: &std::path::Path) -> Option<Self> {
        None
    }

    /// Waits the whole time; nothing is told.
    pub fn wait(&self, timeout: std::time::Duration) -> bool {
        std::thread::sleep(timeout);
        false
    }
}

pub fn background_thread() {}

/// UTC.
pub fn utc_offset(_unix: i64) -> i64 {
    0
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];

/// The date as `d MMM`, e.g. `11 Nov` (UTC).
pub fn local_date(unix: i64) -> String {
    // "2026-11-11T…" from the RFC 3339 form.
    let iso = crate::timefmt::rfc3339(unix);
    let month: usize = iso.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
    let day: u32 = iso.get(8..10).and_then(|d| d.parse().ok()).unwrap_or(1);
    format!("{day} {}", MONTHS.get(month.saturating_sub(1)).unwrap_or(&"Jan"))
}

/// The time as `14:10`, or `Thu 14:10` with the weekday (UTC).
pub fn local_time(unix: i64, with_day: bool) -> String {
    let iso = crate::timefmt::rfc3339(unix);
    let time = iso.get(11..16).unwrap_or_default();
    if with_day {
        // 1970-01-01 was a Thursday.
        let day = WEEKDAYS.get(unix.div_euclid(86_400).rem_euclid(7) as usize).unwrap_or(&"");
        format!("{day} {time}")
    } else {
        time.to_string()
    }
}

/// The display language is German: `LANG` says so.
pub fn system_language_is_german() -> bool {
    std::env::var("LANG").is_ok_and(|l| l.starts_with("de"))
}

/// HTTPS: not available here yet.
pub mod http {
    pub use crate::os::Response;

    const MISSING: &str = "HTTPS isn't available on this system yet";

    pub fn get(_host: &str, _path: &str, _headers: &[(&str, &str)]) -> Result<Response, String> {
        Err(MISSING.into())
    }

    pub fn post(
        _to: (&str, u16, bool),
        _path: &str,
        _headers: &[(&str, &str)],
        _body: &[u8],
    ) -> Result<Response, String> {
        Err(MISSING.into())
    }
}

/// TLS: not available here yet.
pub mod tls {
    use std::io::{self, Read, Write};
    use std::net::TcpStream;

    /// A TLS connection; it can't be made here.
    pub struct TlsStream(TcpStream);

    impl TlsStream {
        pub fn connect(_tcp: TcpStream, _host: &str) -> io::Result<TlsStream> {
            Err(io::Error::other("TLS isn't available on this system yet"))
        }
    }

    impl Read for TlsStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for TlsStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
}

/// PATH as it is now: only the one Trayward started with is known here.
pub fn current_path() -> Option<std::ffi::OsString> {
    None
}

/// Secrets: no store here, so nothing is kept.
pub mod secret {
    pub fn read(_target: &str) -> Option<String> {
        None
    }

    pub fn write(_target: &str, _user: &str, _secret: &str) -> Result<(), String> {
        Err("no credential store on this system".into())
    }

    pub fn delete(_target: &str) -> Result<(), String> {
        Ok(())
    }

    pub fn list(_prefix: &str) -> Vec<String> {
        Vec::new()
    }
}
