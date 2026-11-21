//! Small Win32 helpers of the app: cross-thread window messages, windows and
//! their titles, the clipboard, input, and handing a hook event to the running
//! app. Processes, running programs and the clock are the core's
//! (`trayward_core::os`), re-exported here.

use std::any::Any;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_APP,
};

pub use trayward_core::os::{
    ProcEntry, background_thread, now_ms, process_alive, process_image, process_start_time, process_table, started_by,
    system_program, trim_heap,
};

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

/// Tells a window that a value waits for it in [`take_posted`]. The message
/// itself carries nothing: any program can post a message to a window, so a
/// pointer in one couldn't be trusted.
pub const WM_POSTED: u32 = WM_APP + 100;

/// A value on its way to a window's thread.
struct Posted {
    token: u64,
    hwnd: isize,
    kind: u32,
    value: Box<dyn Any + Send>,
}

static POSTED: Mutex<VecDeque<Posted>> = Mutex::new(VecDeque::new());

fn posted() -> std::sync::MutexGuard<'static, VecDeque<Posted>> {
    POSTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Hands `value` to the thread of `hwnd`, which gets [`WM_POSTED`] and takes it
/// with [`take_posted`], one value per message, in the order they were sent.
/// `kind` says what it is (and so its type).
pub fn post_boxed<T: Send + 'static>(hwnd: SendHwnd, kind: u32, value: T) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    posted().push_back(Posted { token, hwnd: hwnd.0, kind, value: Box::new(value) });
    // SAFETY: posting a message without data to a window handle; it may be gone.
    if unsafe { PostMessageW(Some(hwnd.get()), WM_POSTED, WPARAM(0), LPARAM(0)) }.is_err() {
        // The window is gone (or its queue full): nobody would ever take the value.
        posted().retain(|p| p.token != token);
    }
}

/// What a reader of the core hands its results to: [`post_boxed`] to `hwnd` as `kind`.
pub fn poster<T: Send + 'static>(hwnd: SendHwnd, kind: u32) -> impl Fn(T) + Send + 'static {
    move |value| post_boxed(hwnd, kind, value)
}

/// The oldest value sent to `hwnd` with [`post_boxed`] and its kind; `None` when
/// nothing waits (a [`WM_POSTED`] that wasn't ours).
pub fn take_posted(hwnd: HWND) -> Option<(u32, Box<dyn Any + Send>)> {
    let mut queue = posted();
    let at = queue.iter().position(|p| p.hwnd == hwnd.0 as isize)?;
    queue.remove(at).map(|p| (p.kind, p.value))
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
    let title = buf.get(..usize::try_from(len).unwrap_or(0)).unwrap_or_default();
    Some((hwnd.0 as isize, pid, String::from_utf16_lossy(title)))
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
                    let title = buf.get(..usize::try_from(len).unwrap_or(0)).unwrap_or_default();
                    find.titles.push((SendHwnd::new(hwnd), String::from_utf16_lossy(title)));
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

/// Empties the clipboard (after a token was taken from it).
pub fn empty_clipboard(owner: HWND) {
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard};
    // SAFETY: the clipboard is opened, emptied and closed here.
    unsafe {
        if OpenClipboard(Some(owner)).is_ok() {
            let _ = EmptyClipboard();
            let _ = CloseClipboard();
        }
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

/// `dwData` tag of the WM_COPYDATA message carrying a hook event (`trayward_core::hook::HookMessage`).
pub const HOOK_COPYDATA_TAG: usize = 0x4354_4231; // "CTB1"

/// Hands a hook event to the running app's control window (hook mode, see
/// `trayward_core::hook::run`); nothing when the app isn't running.
pub fn send_hook(bytes: &[u8]) {
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_COPYDATA,
    };
    use windows::core::PCWSTR;
    let class = crate::app::CONTROL_CLASS;
    // SAFETY: FindWindowW with a static class name; COPYDATASTRUCT points into
    // `bytes`, which outlives the synchronous SendMessageTimeoutW call.
    unsafe {
        let Ok(hwnd) = FindWindowW(class, PCWSTR::null()) else { return };
        // The window must be this program's own (the app runs from the exe the hooks
        // name): any program could make a window of that class, to be handed every
        // session's folder and prompts. Both paths come the same way, so they match
        // however the exe was started; when either can't be read, it's sent.
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if let (Some(theirs), Some(ours)) = (process_image(pid), process_image(std::process::id())) {
            if !theirs.as_os_str().eq_ignore_ascii_case(ours.as_os_str()) {
                return;
            }
        }
        let cds =
            COPYDATASTRUCT { dwData: HOOK_COPYDATA_TAG, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut _ };
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const COPYDATASTRUCT as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            2000,
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posted_values_wait_for_their_window_in_order() {
        // Without a window the wake-up goes to this thread's own queue; the values wait here.
        let here = SendHwnd::new(HWND(std::ptr::null_mut()));
        post_boxed(here, 7, String::from("first"));
        post_boxed(here, 8, 2u32);
        let (kind, value) = take_posted(here.get()).unwrap();
        assert_eq!((kind, value.downcast::<String>().ok().as_deref()), (7, Some(&String::from("first"))));
        let (kind, value) = take_posted(here.get()).unwrap();
        // A value isn't taken for another type than it has.
        assert_eq!(kind, 8);
        assert!(value.downcast::<String>().is_err());
        // A wake-up nobody sent (any program can post one) finds nothing.
        assert!(take_posted(here.get()).is_none());
        // A window that doesn't exist: nothing is kept for it.
        let gone = SendHwnd::new(HWND(0xDEAD_BEE0_usize as *mut c_void));
        post_boxed(gone, 7, 1u8);
        assert!(take_posted(gone.get()).is_none());
    }
}
