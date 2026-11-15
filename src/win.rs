//! Small Win32 helpers shared by the rest of the app: strings, cross-thread
//! window messages and process inspection.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
};
use windows::core::PWSTR;

/// A window handle that can be moved to another thread. `HWND` wraps a raw
/// pointer and is not `Send`, but window handles are valid process-wide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SendHwnd(isize);

impl SendHwnd {
    pub fn new(h: HWND) -> Self {
        Self(h.0 as isize)
    }
    pub fn get(self) -> HWND {
        HWND(self.0 as *mut c_void)
    }
}

/// Posts `value` to `hwnd` as a boxed pointer in `lParam`. The window
/// procedure must take it back with [`take_boxed`] using the same type.
pub fn post_boxed<T: Send + 'static>(hwnd: SendHwnd, msg: u32, value: T) {
    let ptr = Box::into_raw(Box::new(value));
    // SAFETY: on success, ownership passes to the receiver; on failure we reclaim it.
    if unsafe { PostMessageW(Some(hwnd.get()), msg, WPARAM(0), LPARAM(ptr as isize)) }.is_err() {
        drop(unsafe { Box::from_raw(ptr) });
    }
}

/// Takes back a value sent with [`post_boxed`].
///
/// # Safety
/// `lp` must come from `post_boxed::<T>` with the same `T`, and be taken once.
pub unsafe fn take_boxed<T>(lp: LPARAM) -> T {
    *unsafe { Box::from_raw(lp.0 as *mut T) }
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
/// A process we may not inspect counts as alive.
pub fn process_alive(pid: u32, started: Option<u64>) -> bool {
    if crate::demo::active() && pid >= crate::demo::PID_BASE {
        return true;
    }
    let h = match open_process(pid) {
        Ok(h) => h,
        Err(e) => return e.code() == ERROR_ACCESS_DENIED.to_hresult(),
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
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

pub struct ProcEntry {
    pub parent: u32,
    /// Executable file name, e.g. `claude.exe`.
    pub exe: String,
}

/// The window in front: its handle, process id and title.
pub fn foreground_window() -> Option<(isize, u32, String)> {
    let mut buf = [0u16; 512];
    let mut pid = 0u32;
    // SAFETY: plain queries with valid out-pointers; the title is read from
    // the window's cached caption, without sending it a message.
    let (hwnd, len) = unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (hwnd, GetWindowTextW(hwnd, &mut buf))
    };
    Some((hwnd.0 as isize, pid, String::from_utf16_lossy(&buf[..len.max(0) as usize])))
}

/// The titles of the visible top-level windows of process `pid`.
pub fn window_titles(pid: u32) -> Vec<String> {
    windows_of(pid).into_iter().map(|(_, title)| title).collect()
}

/// The visible top-level windows of process `pid` that have a title: handle and title.
pub fn windows_of(pid: u32) -> Vec<(SendHwnd, String)> {
    struct Find {
        pid: u32,
        titles: Vec<(SendHwnd, String)>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> windows::core::BOOL {
        // SAFETY: `lp` points to the `Find` below, alive for the whole EnumWindows call.
        let find = unsafe { &mut *(lp.0 as *mut Find) };
        let mut owner = 0u32;
        let mut buf = [0u16; 512];
        // SAFETY: plain queries with valid out-pointers; the title comes from
        // the window's cached caption, without sending it a message.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            if owner == find.pid && IsWindowVisible(hwnd).as_bool() {
                let len = GetWindowTextW(hwnd, &mut buf);
                if len > 0 {
                    find.titles.push((SendHwnd::new(hwnd), String::from_utf16_lossy(&buf[..len as usize])));
                }
            }
        }
        true.into()
    }
    let mut find = Find { pid, titles: Vec::new() };
    // SAFETY: the callback only uses `find` during this call.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut find as *mut Find as isize)) };
    find.titles
}

/// What a program run with [`run`] left behind.
pub struct Output {
    /// It ended by itself with exit code 0.
    pub success: bool,
    /// It ended by itself and all it wrote was read. When not (it took too
    /// long and was stopped, or a program it started holds its output open),
    /// `stdout` and `stderr` hold what came until then.
    pub complete: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Runs a command-line tool without a console window and returns what it
/// wrote, or why it couldn't be started. One that takes longer than
/// `timeout` is stopped (see [`Output::complete`]). Blocking: never on the UI
/// thread. `program` should be a full path (see [`find_program`]).
///
/// Its output is read on threads while it runs: a tool that writes more than
/// a pipe holds would otherwise wait forever for someone to read, and a
/// program it started may keep the pipe open after it ended.
pub fn run(program: &std::ffi::OsStr, args: &[&str], timeout: std::time::Duration) -> Result<Output, String> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let name = std::path::Path::new(program).file_stem().unwrap_or(program).to_string_lossy().into_owned();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("{name}: {e}"))?;
    type Pipe = (Arc<Mutex<Vec<u8>>>, std::sync::mpsc::Receiver<()>);
    // What a pipe gave so far, and a signal once it's closed.
    fn reader(pipe: Option<impl Read + Send + 'static>) -> Pipe {
        let (got, (tx, rx)) = (Arc::new(Mutex::new(Vec::new())), std::sync::mpsc::channel());
        if let Some(mut pipe) = pipe {
            let got = got.clone();
            std::thread::spawn(move || {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    got.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk[..n]);
                }
                let _ = tx.send(());
            });
        } else {
            let _ = tx.send(());
        }
        (got, rx)
    }
    let (stdout, stderr) = (reader(child.stdout.take()), reader(child.stderr.take()));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => break None,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name}: {e}"));
            }
        }
    };
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    // Ended: its output is all there once the pipes close (unless something it started holds them).
    let left = || match status {
        Some(_) => timeout.saturating_sub(start.elapsed()).max(Duration::from_millis(200)),
        None => Duration::from_millis(200),
    };
    let closed = [&stdout, &stderr].map(|(_, done)| done.recv_timeout(left()).is_ok());
    let take = |(got, _): &Pipe| std::mem::take(&mut *got.lock().unwrap_or_else(|e| e.into_inner()));
    Ok(Output {
        success: status.is_some_and(|s| s.success()),
        complete: status.is_some() && closed == [true, true],
        stdout: take(&stdout),
        stderr: take(&stderr),
    })
}

/// A program of Windows itself (`wsl.exe`, `explorer.exe`), by its full path
/// in the Windows folder: never a file of that name next to Trayward's exe or
/// in the working folder.
pub fn system_program(name: &str) -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()));
    [root.join("System32").join(name), root.join(name)].into_iter().find(|p| p.is_file())
}

/// A tool the user installed (`git`, `gh`, `glab`), by its full path from
/// the folders in PATH. Windows would look in the folder of Trayward's exe
/// (often Downloads) first, and in the working folder: not here.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let own = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute() && own.as_deref() != Some(dir.as_path()))
        .flat_map(|dir| ["exe", "cmd", "bat"].map(|ext| dir.join(format!("{name}.{ext}"))))
        .find(|p| p.is_file())
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
                ProcEntry { parent: e.th32ParentProcessID, exe: String::from_utf16_lossy(&e.szExeFile[..len]) },
            );
            ok = Process32NextW(snap.0, &mut e).is_ok();
        }
    }
    table
}

/// Current time as unix milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// The user's Downloads folder (wherever it was moved to).
pub fn downloads_folder() -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: the returned string is freed with CoTaskMemFree after copying.
    let known = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None).ok().map(|p| {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const c_void));
            s
        })
    };
    known
        .flatten()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Downloads"))
}

/// Whether Ctrl is held.
pub fn ctrl_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
    // SAFETY: plain query.
    unsafe { GetKeyState(i32::from(VK_CONTROL.0)) < 0 }
}

/// Seconds since the last mouse or keyboard input.
pub fn idle_secs() -> u64 {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: plain queries with a valid out-pointer; tick counts wrap after 49 days.
    unsafe {
        if !GetLastInputInfo(&mut info).as_bool() {
            return 0;
        }
        u64::from(GetTickCount().wrapping_sub(info.dwTime)) / 1000
    }
}

/// Puts text on the clipboard (best effort).
pub fn set_clipboard_text(owner: HWND, text: &str) {
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    let utf16: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the memory is sized for the string and handed to the clipboard,
    // which owns it after a successful SetClipboardData; otherwise it's freed.
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, utf16.len() * 2) {
            let p = GlobalLock(mem) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(utf16.as_ptr(), p, utf16.len());
                let _ = GlobalUnlock(mem);
                if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).is_err() {
                    let _ = GlobalFree(Some(mem));
                }
            } else {
                let _ = GlobalFree(Some(mem));
            }
        }
        let _ = CloseClipboard();
    }
}

/// Text on the clipboard, if any.
pub fn clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    // SAFETY: the clipboard is opened and closed here; the locked memory is
    // read up to its NUL, and never past its size (another program put it there).
    unsafe {
        OpenClipboard(None).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let mem = HGLOBAL(h.0);
            let p = GlobalLock(mem) as *const u16;
            if p.is_null() {
                return None;
            }
            let most = GlobalSize(mem) / 2;
            let mut len = 0;
            while len < most && *p.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            let _ = GlobalUnlock(mem);
            Some(s)
        });
        let _ = CloseClipboard();
        text
    }
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

/// Lowers the calling thread's CPU and disk priority, for long background work.
pub fn background_thread() {
    // SAFETY: the pseudo handle of the calling thread.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
}
