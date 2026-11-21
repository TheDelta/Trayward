//! Windows: processes (ToolHelp, process times), local time and its formats
//! in the user's language, the display language, HTTPS over WinHTTP and TLS
//! over SChannel.

use super::ProcEntry;
use std::collections::HashMap;
use std::path::PathBuf;
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, HANDLE, SYSTEMTIME};
use windows::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::core::{PCWSTR, PWSTR, w};

pub mod http;
pub mod secret;
pub mod tls;

/// PATH as the registry has it now: the user's folders, then the system's (with
/// `%VARIABLES%` expanded). A tool installed after Trayward started is in it,
/// though not in the PATH Trayward got when it started.
pub fn current_path() -> Option<std::ffi::OsString> {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let read = |root: HKEY, key: PCWSTR| -> Option<String> {
        let mut size = 0u32;
        // SAFETY: first the size, then the value into a buffer of that size; both
        // calls get pointers to live values.
        unsafe {
            RegGetValueW(root, key, w!("Path"), RRF_RT_REG_SZ, None, None, Some(&mut size)).ok().ok()?;
            let mut buf = vec![0u16; (size as usize).div_ceil(2)];
            let data = Some(buf.as_mut_ptr().cast());
            RegGetValueW(root, key, w!("Path"), RRF_RT_REG_SZ, None, data, Some(&mut size)).ok().ok()?;
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(buf.get(..len).unwrap_or_default()))
        }
    };
    let user = read(HKEY_CURRENT_USER, w!("Environment"));
    let system = read(HKEY_LOCAL_MACHINE, w!(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"));
    let both: Vec<String> = [user, system].into_iter().flatten().filter(|p| !p.is_empty()).collect();
    (!both.is_empty()).then(|| both.join(";").into())
}

/// Closes a kernel handle when dropped.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn open_process(pid: u32) -> windows::core::Result<OwnedHandle> {
    // SAFETY: plain query-only open; the handle is closed by OwnedHandle.
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map(OwnedHandle)
}

fn filetime_u64(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

fn creation_time(h: &OwnedHandle) -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    // SAFETY: valid handle and out-pointers.
    unsafe { GetProcessTimes(h.0, &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
    Some(filetime_u64(created))
}

/// When the process started (FILETIME ticks). Together with the pid this
/// identifies a process even after Windows reuses the pid.
pub fn process_start_time(pid: u32) -> Option<u64> {
    creation_time(&open_process(pid).ok()?)
}

/// Whether the process `pid` is still running and, if `started` is given,
/// is the same process that started then (not a later one with a reused pid).
/// A process we may not inspect counts as alive when nothing more is known of
/// it. With `started` it doesn't: that came from a process of the user's own,
/// which can be inspected, so this is another one with its pid (after a restart
/// of the PC, a service's).
pub fn process_alive(pid: u32, started: Option<u64>) -> bool {
    if crate::demo::active() && pid >= crate::demo::PID_BASE {
        return true;
    }
    let h = match open_process(pid) {
        Ok(h) => h,
        Err(e) => return started.is_none() && e.code() == ERROR_ACCESS_DENIED.to_hresult(),
    };
    const STILL_ACTIVE: u32 = 259;
    let mut code = 0u32;
    // SAFETY: valid handle and out-pointer.
    if unsafe { GetExitCodeProcess(h.0, &mut code) }.is_err() {
        return true;
    }
    if code != STILL_ACTIVE {
        return false;
    }
    match started {
        Some(s) => creation_time(&h).is_none_or(|c| c == s),
        None => true,
    }
}

/// Whether the process `pid` started no later than `unix_secs` (a few
/// seconds of slack). A file that names a pid (a lock) was written by that
/// process only if so: Windows reuses pids. Unknown counts as yes.
pub fn started_by(pid: u32, unix_secs: i64) -> bool {
    // FILETIME ticks (100 ns since 1601) to unix seconds.
    let started = process_start_time(pid).map(|t| (t / 10_000_000) as i64 - 11_644_473_600);
    started.is_none_or(|s| s <= unix_secs + 5)
}

/// Full path of a running process's executable.
pub fn process_image(pid: u32) -> Option<PathBuf> {
    let h = open_process(pid).ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: buffer and length describe valid memory.
    unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(buf.get(..len as usize)?)))
}

/// A snapshot of all processes: pid -> parent pid and exe name.
pub fn process_table() -> HashMap<u32, ProcEntry> {
    let mut table = HashMap::new();
    // SAFETY: snapshot handle is closed by OwnedHandle; the entry struct is sized correctly.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return table;
        };
        let snap = OwnedHandle(snap);
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap.0, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            table.insert(
                e.th32ProcessID,
                ProcEntry {
                    parent: e.th32ParentProcessID,
                    exe: String::from_utf16_lossy(e.szExeFile.get(..len).unwrap_or(&e.szExeFile)),
                },
            );
            ok = Process32NextW(snap.0, &mut e).is_ok();
        }
    }
    table
}

/// Gives memory freed on the process heap back to Windows.
pub fn trim_heap() {
    use windows::Win32::System::Memory::{GetProcessHeap, HEAP_FLAGS, HeapCompact};
    // SAFETY: the process heap, which lives as long as the process.
    unsafe {
        if let Ok(heap) = GetProcessHeap() {
            HeapCompact(heap, HEAP_FLAGS(0));
        }
    }
}

/// Changes in a folder (files added, removed, renamed or written), told by
/// Windows: a thread waiting on it costs nothing until something changes.
pub struct DirWatch(HANDLE);

impl DirWatch {
    /// Watches `dir` (not its subfolders); `None` if it can't be watched.
    pub fn new(dir: &std::path::Path) -> Option<Self> {
        use windows::Win32::Storage::FileSystem::{
            FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
            FindFirstChangeNotificationW,
        };
        let path = windows::core::HSTRING::from(dir.as_os_str());
        let what = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
        // SAFETY: a valid wide string; the handle is closed in Drop.
        unsafe { FindFirstChangeNotificationW(&path, false, what) }.ok().map(Self)
    }

    /// Waits up to `timeout` for a change; true if one came.
    pub fn wait(&self, timeout: std::time::Duration) -> bool {
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::Storage::FileSystem::FindNextChangeNotification;
        use windows::Win32::System::Threading::WaitForSingleObject;
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: our own change handle, open until Drop.
        if unsafe { WaitForSingleObject(self.0, ms) } != WAIT_OBJECT_0 {
            return false;
        }
        // SAFETY: as above; asks for the next change.
        let _ = unsafe { FindNextChangeNotification(self.0) };
        true
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        use windows::Win32::Storage::FileSystem::FindCloseChangeNotification;
        // SAFETY: our own change handle, closed exactly once.
        let _ = unsafe { FindCloseChangeNotification(self.0) };
    }
}

// SAFETY: a change handle may be waited on and closed from any thread.
unsafe impl Send for DirWatch {}

/// Lowers the calling thread's CPU and disk priority, for long background work.
pub fn background_thread() {
    // SAFETY: the pseudo handle of the calling thread.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
}

/// Offset of local time from UTC at `unix`, in seconds (e.g. 3600 in CET).
pub fn utc_offset(unix: i64) -> i64 {
    let ticks = (unix + 11_644_473_600).max(0) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    // SAFETY: pointers refer to live stack values.
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return 0;
        }
    }
    let l = |v: u16| i64::from(v);
    let local_secs = crate::timefmt::days_from_civil(l(local.wYear), l(local.wMonth), l(local.wDay)) * 86_400
        + l(local.wHour) * 3600
        + l(local.wMinute) * 60
        + l(local.wSecond);
    local_secs - unix
}

/// The local date as `d MMM`, e.g. `11 Nov` (month names in the user's language).
pub fn local_date(unix: i64) -> String {
    let ticks = (unix + 11_644_473_600).max(0) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    let mut buf = [0u16; 64];
    // SAFETY: all pointers refer to live stack values; buffers are sized.
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return String::new();
        }
        let n =
            GetDateFormatEx(locale(), Default::default(), Some(&local), w!("d MMM"), Some(&mut buf), PCWSTR::null());
        written(&buf, n)
    }
}

/// What a Windows call wrote into `buf`: `n` characters, the last of them the end mark.
fn written(buf: &[u16], n: i32) -> String {
    let text = usize::try_from(n).ok().and_then(|n| buf.get(..n.checked_sub(1)?));
    String::from_utf16_lossy(text.unwrap_or_default())
}

/// Local wall-clock time in the user's format, optionally with the weekday:
/// `14:10` or `Thu 14:10`.
pub fn local_time(unix: i64, with_day: bool) -> String {
    let ticks = (unix + 11_644_473_600).max(0) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    let mut buf = [0u16; 64];
    let mut out = String::new();
    // SAFETY: all pointers refer to live stack values; buffers are sized.
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return out;
        }
        if with_day {
            let n =
                GetDateFormatEx(locale(), Default::default(), Some(&local), w!("ddd"), Some(&mut buf), PCWSTR::null());
            if n > 1 {
                out = written(&buf, n) + " ";
            }
        }
        let n = GetTimeFormatEx(locale(), TIME_NOSECONDS, Some(&local), PCWSTR::null(), Some(&mut buf));
        out += &written(&buf, n);
    }
    out
}

/// The locale for dates and times: the user's own with the system language,
/// else one that matches the chosen language.
fn locale() -> PCWSTR {
    use crate::i18n::Choice;
    match crate::i18n::choice() {
        Choice::System => PCWSTR::null(),
        Choice::English => w!("en-GB"),
        Choice::German => w!("de-DE"),
    }
}

/// Windows' display language is German.
pub fn system_language_is_german() -> bool {
    // SAFETY: plain query.
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    // The primary language is the low 10 bits; 0x07 is German.
    id & 0x3FF == 0x07
}

#[cfg(test)]
mod tests {
    /// PATH from the registry: the system's folders are in it, their variables expanded.
    #[test]
    fn reads_path_as_it_is_now() {
        let path = super::current_path().unwrap().to_string_lossy().to_lowercase();
        assert!(path.contains(r"\system32"), "{path}");
        assert!(!path.contains("%system"), "{path}");
    }
}
