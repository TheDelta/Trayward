//! Switching to a session that runs in a terminal: the window of its console
//! and, in Windows Terminal, the tab it runs in.
//!
//! The console is found by attaching to it (`AttachConsole` with the Claude
//! process's id). A classic console window is that window itself. Windows
//! Terminal shows a hidden pseudo console window instead, owned by the
//! terminal's window. To find the tab, the console gets a unique title for a
//! moment, which Windows Terminal shows as the tab's name; UI Automation finds
//! that tab and selects it, and the old title is put back.
//!
//! A terminal inside VS Code (or an editor built on it) has no window of
//! its own: its pseudo console belongs to no window. Such a session is told
//! by VS Code among its parent processes. The same unique title shows in
//! VS Code as the name of its terminal, so UI Automation finds the VS Code
//! window it's in and the terminal itself: the text area of the terminal
//! shown (`Terminal 3, <title>`), or, for one that isn't shown, its entry in
//! the panel's list of terminals (`Terminal 3 <title>`), which is opened.
//! What was found is remembered (window and terminal number) for when the
//! title doesn't show next time; failing all that, the window named after the
//! session's folder comes to the front.
//!
//! Attaching to a console ties a process to it: when the console closes (its
//! tab or window is closed), Windows ends every process attached to it. So
//! Trayward doesn't attach itself: it starts itself once more as a helper
//! ([`ARG`]) that attaches, looks, writes one line saying what it found
//! ([`Looked`]) and ends. If the terminal closes in that second or two, only
//! the helper goes with it.
//!
//! Runs on its own thread: it waits for the helper, which waits for the
//! terminal and calls UI Automation.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{FALSE, HWND, TRUE};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Console::{
    AttachConsole, CTRL_BREAK_EVENT, CTRL_C_EVENT, FreeConsole, GetConsoleTitleW, GetConsoleWindow,
    SetConsoleCtrlHandler, SetConsoleTitleW,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationLegacyIAccessiblePattern, IUIAutomationSelectionItemPattern, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId,
    UIA_ListItemControlTypeId, UIA_NamePropertyId, UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, GA_ROOT, GW_OWNER, GetAncestor, GetClassNameW, GetWindow, IsIconic, IsWindow,
    IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::{BOOL, HSTRING, Interface};

/// Windows Terminal's top-level window class.
const WINDOWS_TERMINAL: &str = "CASCADIA_HOSTING_WINDOW_CLASS";
/// The hidden window a pseudo console (ConPTY) has in place of a console window.
const PSEUDO_CONSOLE: &str = "PseudoConsoleWindow";
/// How long to wait for Windows Terminal to show the new tab name.
const TAB_WAIT: Duration = Duration::from_millis(1500);

/// The argument Trayward starts itself with to look at a terminal: `trayward.exe
/// trayward-terminal <pid> <VS Code's pid or 0> <the folder's window name or "">`.
/// Trayward's own, not for anything else to call: it can change with any version.
pub const ARG: &str = "trayward-terminal";
/// How long the helper gets: it waits for a tab's new name, and going through all
/// of VS Code's windows takes seconds.
const LOOK_TIMEOUT: Duration = Duration::from_secs(20);

/// Editors built on VS Code, by their exe: a terminal of theirs runs inside them.
const VSCODE_EXES: [&str; 5] = ["Code.exe", "Code - Insiders.exe", "Cursor.exe", "Windsurf.exe", "VSCodium.exe"];

/// The VS Code process (its main one) that `pid` runs in, if it runs in a
/// terminal of VS Code: an ancestor with VS Code's exe, the topmost of them.
fn vscode_ancestor(table: &std::collections::HashMap<u32, crate::win::ProcEntry>, pid: u32) -> Option<u32> {
    let mut found = None;
    let mut at = pid;
    // A few levels: the shell, VS Code's terminal host, VS Code.
    for _ in 0..12 {
        let Some(e) = table.get(&at) else { break };
        if e.parent == 0 || e.parent == at {
            break;
        }
        at = e.parent;
        match table.get(&at) {
            Some(p) if VSCODE_EXES.iter().any(|x| p.exe.eq_ignore_ascii_case(x)) => found = Some(at),
            // Above VS Code's own processes: no need to look further.
            _ if found.is_some() => break,
            _ => {}
        }
    }
    found
}

/// What the helper found, and the one line it writes for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Looked {
    /// The process has no console to attach to (any more).
    Gone,
    /// Attached, but no window or terminal of it was found.
    Nothing,
    /// The console's window, or the terminal's whose tab was selected.
    Window(isize),
    /// In VS Code: its window and the terminal's number there, now shown.
    Terminal(isize, u32),
}

impl Looked {
    fn line(self) -> String {
        match self {
            Looked::Gone => "gone".into(),
            Looked::Nothing => "none".into(),
            Looked::Window(w) => format!("window {w}"),
            Looked::Terminal(w, n) => format!("terminal {w} {n}"),
        }
    }

    /// What the helper wrote. Nothing at all means it ended before it could
    /// write: its console closed while it was attached, so the terminal is gone.
    fn parse(text: &str) -> Looked {
        let mut words = text.split_whitespace();
        let what = words.next();
        let mut number = || words.next().and_then(|w| w.parse::<isize>().ok()).filter(|&n| n > 0);
        match what {
            Some("none") => Looked::Nothing,
            Some("window") => number().map_or(Looked::Nothing, Looked::Window),
            Some("terminal") => match (number(), number().and_then(|n| u32::try_from(n).ok())) {
                (Some(w), Some(n)) => Looked::Terminal(w, n),
                _ => Looked::Nothing,
            },
            _ => Looked::Gone,
        }
    }
}

/// What the helper does with the events of the console it's attached to. Windows
/// sends them to every program of a console:
///
/// - Ctrl+C and Ctrl+Break are for the programs that run there: nothing.
/// - The console closes: Windows ends the helper, as it would any program there.
unsafe extern "system" fn console_event(kind: u32) -> BOOL {
    if kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT { TRUE } else { FALSE }
}

/// The helper: attaches to the console of process `pid`, finds its window (and tab
/// or terminal), and writes what it found. `args` are the ones after [`ARG`].
/// Returns the exit code.
pub fn helper(args: &[String]) -> i32 {
    use std::io::Write;
    use std::os::windows::io::AsHandle;
    let Some(pid) = args.first().and_then(|a| a.parse::<u32>().ok()) else { return 2 };
    let code = args.get(1).and_then(|a| a.parse::<u32>().ok()).filter(|&c| c != 0);
    let folder = args.get(2).map(String::as_str).filter(|f| !f.is_empty());
    // Where to write, taken before attaching: a console brings its own output along.
    let out = std::io::stdout().as_handle().try_clone_to_owned().map(std::fs::File::from);
    // SAFETY: attaching to and detaching from another process's console; this
    // process does nothing else.
    let found = unsafe {
        let _ = FreeConsole();
        if AttachConsole(pid).is_err() {
            Looked::Gone
        } else {
            // After attaching: a handler set before is dropped by it.
            let _ = SetConsoleCtrlHandler(Some(console_event), true);
            let found = match code {
                // In VS Code: its window that shows this terminal.
                Some(code) => with_marker(|marker| find_in_vscode(code, folder, marker))
                    .flatten()
                    .map_or(Looked::Nothing, |(window, number)| Looked::Terminal(window.0 as isize, number)),
                None => match console_window() {
                    Some((window, tabs)) => {
                        if tabs {
                            with_marker(|marker| find_and_select(window, marker));
                        }
                        Looked::Window(window.0 as isize)
                    }
                    None => Looked::Nothing,
                },
            };
            let _ = FreeConsole();
            found
        }
    };
    if let Ok(mut out) = out {
        let _ = writeln!(out, "{}", found.line());
    }
    0
}

/// Starts the helper for the console of process `pid` and reads what it found.
fn look(pid: u32, code: Option<u32>, folder: Option<&str>) -> Result<Looked, String> {
    let failed = |e: String| crate::i18n::f("Couldn't look at the terminal: {0}", &[&e]);
    let exe = std::env::current_exe().map_err(|e| failed(e.to_string()))?;
    // The helper brings VS Code's window to the front itself. Trayward may (it was
    // just clicked) and passes that on; without it, the window still comes to the
    // front below, only the terminal in it might not get the focus.
    // SAFETY: a plain call.
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let (pid, code) = (pid.to_string(), code.unwrap_or(0).to_string());
    let args = [ARG, pid.as_str(), code.as_str(), folder.unwrap_or("")];
    let out = trayward_core::os::run(exe.as_os_str(), &args, LOOK_TIMEOUT).map_err(failed)?;
    // Stopped because it took too long: nothing found, the terminal may well be there.
    Ok(if out.complete { Looked::parse(&out.stdout_text()) } else { Looked::Nothing })
}

/// The window a helper named, if it is one (still).
fn window_of(handle: isize) -> Option<HWND> {
    let window = HWND(handle as *mut core::ffi::c_void);
    // SAFETY: a plain query; any value may be asked about.
    unsafe { IsWindow(Some(window)) }.as_bool().then_some(window)
}

/// Brings the terminal of the Claude process `pid` to the front, on its tab.
/// `cwd` is the session's folder (in VS Code, its window is looked at first).
pub fn switch_to(pid: u32, cwd: &str) -> Result<(), String> {
    // What VS Code calls the folder's window: its name, or the start of a custom title.
    let folder = (!cwd.is_empty())
        .then(|| crate::vscode::window_name(std::path::Path::new(cwd), crate::vscode::user_window_title().as_deref()));
    let vscode = vscode_ancestor(&crate::win::process_table(), pid);
    let window = match look(pid, vscode, folder.as_deref())? {
        Looked::Gone => {
            return Err(crate::i18n::t("The terminal of this session is gone: the session has ended.").to_string());
        }
        Looked::Nothing => None,
        Looked::Window(window) => window_of(window),
        // Remembered for when its title doesn't show next time.
        Looked::Terminal(window, number) => window_of(window).inspect(|w| remember(pid, *w, number)),
    };
    // In VS Code, but its title didn't show: where it was found before, else
    // at least the window named after the session's folder.
    let window = match (window, vscode) {
        (None, Some(code)) => from_memory(code, pid).or_else(|| folder_window(code, folder.as_deref())),
        (window, _) => window,
    };
    if window.is_none() && vscode.is_some() {
        return Err(
            crate::i18n::t("Couldn't find this session's terminal in VS Code. Is its terminal panel open?").into()
        );
    }
    let window =
        window.ok_or_else(|| crate::i18n::t("Couldn't find the terminal window of this session.").to_string())?;
    // SAFETY: plain window calls on a window that exists (it may close meanwhile: then they fail).
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        if !SetForegroundWindow(window).as_bool() {
            return Err(crate::i18n::t("Windows didn't let Trayward bring the terminal to the front.").into());
        }
    }
    Ok(())
}

/// The window showing the attached console, and whether it's Windows Terminal.
///
/// # Safety
/// The process must be attached to the console.
unsafe fn console_window() -> Option<(HWND, bool)> {
    // SAFETY: plain window queries; the caller attached the console.
    unsafe {
        let console = GetConsoleWindow();
        if console.is_invalid() {
            return None;
        }
        if class_name(console) != PSEUDO_CONSOLE {
            return IsWindowVisible(console).as_bool().then_some((console, false)); // a classic console window
        }
        // A terminal's pseudo console window (it counts as visible, but isn't
        // shown), owned by the terminal's window.
        let owner = GetWindow(console, GW_OWNER).ok()?;
        let root = GetAncestor(owner, GA_ROOT);
        let root = if root.is_invalid() { owner } else { root };
        Some((root, class_name(root) == WINDOWS_TERMINAL))
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    // SAFETY: the buffer's length is passed with it.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(buf.get(..usize::try_from(n).unwrap_or(0)).unwrap_or_default())
}

/// Gives the attached console a unique title for as long as `find` runs
/// (the terminal shows it as its name), then puts the old one back. `None`
/// if the title couldn't be set.
///
/// # Safety
/// The process must be attached to the console.
unsafe fn with_marker<T>(find: impl FnOnce(&str) -> windows::core::Result<T>) -> Option<T> {
    let mut old = vec![0u16; 1024];
    // SAFETY: the buffer's length is passed with it; the caller attached the console.
    let len = unsafe { GetConsoleTitleW(&mut old) } as usize;
    old.truncate(len.min(old.len()));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let marker = format!("Trayward {:08x}", nanos ^ std::process::id());
    // SAFETY: setting the attached console's title, put back below.
    if unsafe { SetConsoleTitleW(&HSTRING::from(marker.as_str())) }.is_err() {
        return None;
    }
    let found = find(&marker).ok();
    old.push(0);
    // SAFETY: a NUL-terminated title.
    let _ = unsafe { SetConsoleTitleW(windows::core::PCWSTR(old.as_ptr())) };
    found
}

/// Waits for VS Code (process `code`) to name a terminal after `marker`: in
/// one of its windows, the terminal's text area is named
/// `Terminal 3, <title> …`. Focuses that terminal and returns its window.
fn find_in_vscode(code: u32, folder: Option<&str>, marker: &str) -> windows::core::Result<Option<(HWND, u32)>> {
    // SAFETY: COM on this thread (the caller's own thread), and UI Automation
    // calls on valid interfaces.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let (cache, terminals) = terminal_query(&automation)?;
        // The window named after the session's folder most likely has the
        // terminal: only it at first (VS Code takes a moment to show the new
        // name), then all of them once (going through all takes seconds).
        let all = crate::win::windows_of(code);
        let likely: Vec<_> = all
            .iter()
            .filter(|(_, title)| folder.is_some_and(|f| crate::vscode::title_is(title, f)))
            .cloned()
            .collect();
        let start = Instant::now();
        let mut looked_at_all = false;
        while !looked_at_all {
            let windows = if start.elapsed() < LIKELY_FIRST && !likely.is_empty() {
                &likely
            } else {
                looked_at_all = true;
                &all
            };
            for (window, _) in windows.iter().cloned() {
                let window = window.get();
                let Ok(root) = automation.ElementFromHandle(window) else { continue };
                let Ok(found) = root.FindAllBuildCache(TreeScope_Descendants, &terminals, &cache) else { continue };
                for i in 0..found.Length().unwrap_or(0) {
                    let Ok(e) = found.GetElement(i) else { continue };
                    let Some(number) = e.CachedName().ok().and_then(|n| terminal_number(&n.to_string(), Some(marker)))
                    else {
                        continue;
                    };
                    bring_to_front(window);
                    show_terminal(&automation, &root, &e, number);
                    return Ok(Some((window, number)));
                }
            }
            if !looked_at_all {
                std::thread::sleep(Duration::from_millis(60));
            }
        }
        Ok(None)
    }
}

/// What finds VS Code's terminals: text areas and list entries, with their
/// type and name read along (UI Automation filters them itself: a VS Code
/// window has thousands of elements, and reading every one's name takes seconds).
///
/// # Safety
/// COM must be initialised on this thread.
unsafe fn terminal_query(
    automation: &IUIAutomation,
) -> windows::core::Result<(windows::Win32::UI::Accessibility::IUIAutomationCacheRequest, IUIAutomationCondition)> {
    use windows::Win32::System::Variant::VARIANT;
    // SAFETY: plain UI Automation calls on a valid interface.
    unsafe {
        let cache = automation.CreateCacheRequest()?;
        cache.AddProperty(UIA_ControlTypePropertyId)?;
        cache.AddProperty(UIA_NamePropertyId)?;
        let edits =
            automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_EditControlTypeId.0))?;
        let items = automation
            .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ListItemControlTypeId.0))?;
        Ok((cache, automation.CreateOrCondition(&edits, &items)?))
    }
}

/// Shows terminal `number` (its element `e` was found in `root`): an entry
/// of the list of terminals is opened first; then its text area gets the focus.
///
/// # Safety
/// COM must be initialised on this thread; `e` was found with [`terminal_query`]'s cache.
unsafe fn show_terminal(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    e: &IUIAutomationElement,
    number: u32,
) {
    // SAFETY: plain UI Automation calls on valid interfaces.
    unsafe {
        let is_entry = e.CachedControlType().is_ok_and(|t| t == UIA_ListItemControlTypeId);
        if !is_entry {
            // Its text area: focusing it is all it takes.
            let _ = e.SetFocus();
            return;
        }
        // An entry of the list: opened the way the list allows.
        let opened = e
            .GetCurrentPattern(UIA_SelectionItemPatternId)
            .and_then(|p| p.cast::<IUIAutomationSelectionItemPattern>())
            .and_then(|p| p.Select())
            .or_else(|_| {
                e.GetCurrentPattern(UIA_InvokePatternId)
                    .and_then(|p| p.cast::<IUIAutomationInvokePattern>())
                    .and_then(|p| p.Invoke())
            })
            .or_else(|_| {
                e.GetCurrentPattern(UIA_LegacyIAccessiblePatternId)
                    .and_then(|p| p.cast::<IUIAutomationLegacyIAccessiblePattern>())
                    .and_then(|p| p.DoDefaultAction())
            });
        if opened.is_err() {
            let _ = e.SetFocus();
        }
        // Now shown: its text area takes the focus.
        let Ok((cache, terminals)) = terminal_query(automation) else { return };
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(800) {
            if let Ok(found) = root.FindAllBuildCache(TreeScope_Descendants, &terminals, &cache) {
                for i in 0..found.Length().unwrap_or(0) {
                    let Ok(area) = found.GetElement(i) else { continue };
                    let is_area = area.CachedControlType().is_ok_and(|t| t == UIA_EditControlTypeId);
                    let named = area.CachedName().ok().and_then(|n| terminal_number(&n.to_string(), None));
                    if is_area && named == Some(number) {
                        let _ = area.SetFocus();
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(60));
        }
    }
}

/// Where each session's terminal in VS Code was found: the Claude process,
/// the window and the terminal's number. Kept while Trayward runs.
static FOUND: Mutex<Vec<(u32, isize, u32)>> = Mutex::new(Vec::new());

fn remember(pid: u32, window: HWND, number: u32) {
    let mut found = FOUND.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    found.retain(|(p, _, _)| *p != pid);
    found.push((pid, window.0 as isize, number));
    // A few dozen at most: sessions come and go.
    if found.len() > 64 {
        found.remove(0);
    }
}

/// The terminal of `pid` where it was found before, if its window is still
/// one of VS Code's (process `code`): brought to the front, the terminal shown.
fn from_memory(code: u32, pid: u32) -> Option<HWND> {
    let (window, number) = FOUND
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(p, _, _)| *p == pid)
        .map(|(_, w, n)| (*w, *n))?;
    let window = crate::win::windows_of(code).into_iter().map(|(w, _)| w.get()).find(|w| w.0 as isize == window)?;
    bring_to_front(window);
    // SAFETY: COM on this thread, and UI Automation calls on valid interfaces.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let (cache, terminals) = terminal_query(&automation).ok()?;
        let root = automation.ElementFromHandle(window).ok()?;
        let found = root.FindAllBuildCache(TreeScope_Descendants, &terminals, &cache).ok()?;
        // Its text area if it's shown, else its entry in the list.
        let mut entry = None;
        for i in 0..found.Length().unwrap_or(0) {
            let Ok(e) = found.GetElement(i) else { continue };
            if e.CachedName().ok().and_then(|n| terminal_number(&n.to_string(), None)) != Some(number) {
                continue;
            }
            if e.CachedControlType().is_ok_and(|t| t == UIA_EditControlTypeId) {
                let _ = e.SetFocus();
                return Some(window);
            }
            entry = Some(e);
        }
        if let Some(e) = entry {
            show_terminal(&automation, &root, &e, number);
        }
    }
    Some(window)
}

/// The VS Code window (of process `code`) named after the session's folder, brought to the front.
fn folder_window(code: u32, folder: Option<&str>) -> Option<HWND> {
    let folder = folder?;
    let window =
        crate::win::windows_of(code).into_iter().find(|(_, title)| crate::vscode::title_is(title, folder))?.0.get();
    bring_to_front(window);
    Some(window)
}

/// How long only the window named after the session's folder is looked at
/// (VS Code takes a moment to show the new name), before all are, once.
const LIKELY_FIRST: Duration = Duration::from_millis(1500);

/// The number of the terminal VS Code names `name`: its text area
/// (`Terminal 3, <title> Run the command: …`) or its entry in the list of
/// terminals (`Terminal 3 <title>`). With `marker`, only if the title starts with it.
fn terminal_number(name: &str, marker: Option<&str>) -> Option<u32> {
    let rest = name.strip_prefix("Terminal ")?;
    let after = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let number = rest.strip_suffix(after)?.parse().ok()?;
    let title = after.strip_prefix(", ").or_else(|| after.strip_prefix(' '))?;
    marker.is_none_or(|m| title.starts_with(m)).then_some(number)
}

/// Brings `window` to the front, restored if it was minimised.
fn bring_to_front(window: HWND) {
    // SAFETY: plain window calls (the window may close meanwhile: then they fail).
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        let _ = SetForegroundWindow(window);
    }
}

/// Waits for the tab named `name` in `window` and selects it.
fn find_and_select(window: HWND, name: &str) -> windows::core::Result<bool> {
    // SAFETY: COM on this thread (the caller's own thread), and UI Automation
    // calls on valid interfaces.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = automation.ElementFromHandle(window)?;
        let cache = automation.CreateCacheRequest()?;
        cache.AddProperty(UIA_ControlTypePropertyId)?;
        cache.AddProperty(UIA_NamePropertyId)?;
        let all = automation.CreateTrueCondition()?;
        let start = Instant::now();
        while start.elapsed() < TAB_WAIT {
            let found = root.FindAllBuildCache(TreeScope_Descendants, &all, &cache)?;
            for i in 0..found.Length()? {
                let e = found.GetElement(i)?;
                if e.CachedControlType()? == UIA_TabItemControlTypeId && e.CachedName()? == name {
                    let item: IUIAutomationSelectionItemPattern =
                        e.GetCurrentPattern(UIA_SelectionItemPatternId)?.cast()?;
                    item.Select()?;
                    return Ok(true);
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        Ok(false)
    }
}

/// German translations of its messages (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("Couldn't find this session's terminal in VS Code. Is its terminal panel open?", "Das Terminal dieser Sitzung wurde in VS Code nicht gefunden. Ist der Terminalbereich offen?"),
    ("Couldn't look at the terminal: {0}", "Das Terminal konnte nicht angesehen werden: {0}"),
    ("The terminal of this session is gone: the session has ended.", "Das Terminal dieser Sitzung ist weg: Die Sitzung wurde beendet."),
    ("Couldn't find the terminal window of this session.", "Das Terminalfenster dieser Sitzung wurde nicht gefunden."),
    ("Windows didn't let Trayward bring the terminal to the front.", "Windows hat Trayward nicht erlaubt, das Terminal nach vorne zu holen."),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::ProcEntry;
    use std::collections::HashMap;

    fn table(rows: &[(u32, u32, &str)]) -> HashMap<u32, ProcEntry> {
        rows.iter().map(|&(pid, parent, exe)| (pid, ProcEntry { parent, exe: exe.into() })).collect()
    }

    #[test]
    fn tells_a_terminal_inside_vscode() {
        // claude → pwsh → VS Code's terminal host → VS Code → explorer.
        let t = table(&[
            (10, 9, "claude.exe"),
            (9, 8, "pwsh.exe"),
            (8, 7, "Code.exe"),
            (7, 1, "Code.exe"),
            (1, 0, "explorer.exe"),
        ]);
        assert_eq!(vscode_ancestor(&t, 10), Some(7));
        // In Windows Terminal: no VS Code above it.
        let t =
            table(&[(10, 9, "claude.exe"), (9, 8, "pwsh.exe"), (8, 1, "WindowsTerminal.exe"), (1, 0, "explorer.exe")]);
        assert_eq!(vscode_ancestor(&t, 10), None);
        // Cursor, and a parent that's gone.
        let t = table(&[(10, 9, "claude.exe"), (9, 5, "bash.exe"), (5, 4, "Cursor.exe")]);
        assert_eq!(vscode_ancestor(&t, 10), Some(5));
        assert_eq!(vscode_ancestor(&table(&[(10, 99, "claude.exe")]), 10), None);
        // A loop in the table ends.
        assert_eq!(vscode_ancestor(&table(&[(10, 11, "a.exe"), (11, 10, "b.exe")]), 10), None);
    }

    #[test]
    fn finds_the_terminal_by_its_name() {
        let m = Some("Trayward 1a2b3c4d");
        // Its text area, shown.
        let area = "Terminal 7, Trayward 1a2b3c4d Run the command: Toggle Screen Reader Accessibility Mode";
        assert_eq!(terminal_number(area, m), Some(7));
        // Its entry in the list of terminals, also while another is shown.
        assert_eq!(terminal_number("Terminal 12 Trayward 1a2b3c4d", m), Some(12));
        // Another terminal, or not a terminal.
        assert_eq!(terminal_number("Terminal 7, ◐ Fix the cart Run the command: …", m), None);
        assert_eq!(terminal_number("Terminal x, Trayward 1a2b3c4d", m), None);
        assert_eq!(terminal_number("Trayward 1a2b3c4d", m), None);
        assert_eq!(terminal_number("Terminal (Ctrl+ö)", m), None);
        // By its number alone, for the one found before.
        assert_eq!(terminal_number("Terminal 4 bash", None), Some(4));
        assert_eq!(terminal_number("Terminal 4, pwsh Run the command: …", None), Some(4));
        assert_eq!(terminal_number("Terminal 4", None), None);
    }

    #[test]
    fn remembers_where_a_terminal_was() {
        remember(1, HWND(std::ptr::dangling_mut()), 3);
        remember(1, HWND(std::ptr::dangling_mut()), 5);
        let found = FOUND.lock().unwrap().iter().filter(|(p, _, _)| *p == 1).map(|(_, _, n)| *n).collect::<Vec<_>>();
        assert_eq!(found, [5]);
    }

    #[test]
    fn reads_what_the_helper_wrote() {
        for found in [Looked::Gone, Looked::Nothing, Looked::Window(0x2_0a4c), Looked::Terminal(0x7_01f2, 12)] {
            assert_eq!(Looked::parse(&format!("{}\r\n", found.line())), found);
        }
        // It ended with its console before it could write: the terminal is gone.
        assert_eq!(Looked::parse(""), Looked::Gone);
        assert_eq!(Looked::parse("something else"), Looked::Gone);
        // Cut off, or not a window: nothing found.
        assert_eq!(Looked::parse("window"), Looked::Nothing);
        assert_eq!(Looked::parse("window 0"), Looked::Nothing);
        assert_eq!(Looked::parse("window -4"), Looked::Nothing);
        assert_eq!(Looked::parse("terminal 1234"), Looked::Nothing);
        assert_eq!(Looked::parse("terminal 1234 x"), Looked::Nothing);
    }
}
