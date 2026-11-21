//! VS Code windows: which one a session belongs to, its name, and bringing a
//! session's tab to the front.
//!
//! 1. Claude Code's VS Code extension writes a lock file per window to
//!    `~/.claude/ide/*.lock` listing its workspace folders (for WSL windows,
//!    in the distribution's `~/.claude`). The folder that contains the
//!    session's working directory tells us the window.
//! 2. Starting VS Code's exe with that folder (or, for WSL, its
//!    `vscode-remote://wsl+<distro>/…` URI) makes VS Code focus the window that
//!    already has it open.
//! 3. `<scheme>://anthropic.claude-code/open?session=<id>` then makes the
//!    extension reveal that session's tab in the focused window.
//!
//! The URI goes to whichever VS Code window is active, and the extension opens
//! a second copy of a session it doesn't have, so step 3 only runs once the
//! right window is known to be in front.

use crate::{registry, win};
use serde::Deserialize;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, GetForegroundWindow, GetWindowThreadProcessId,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdeLock {
    pid: u32,
    #[serde(default)]
    workspace_folders: Vec<String>,
}

/// What to switch to, copied from the session.
#[derive(Clone, Debug)]
pub struct Target {
    pub session_id: String,
    pub cwd: String,
    /// VS Code's main process, if a hook reported it.
    pub ide_pid: Option<u32>,
    /// WSL distribution, or `None` for Windows.
    pub distro: Option<String>,
}

fn normalize(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Whether `dir` is `folder` or inside it.
fn contains(folder: &str, dir: &str) -> bool {
    let (f, d) = (normalize(folder), normalize(dir));
    !f.is_empty() && (d == f || d.strip_prefix(&f).is_some_and(|rest| rest.starts_with('\\')))
}

/// The window (by its lock) and folder holding `cwd`: the deepest matching
/// workspace folder wins. With `single_only`, multi-root windows are skipped
/// (VS Code only focuses a window by folder when that folder is all it has).
fn find_folder<'a>(locks: &'a [IdeLock], cwd: &str, single_only: bool) -> Result<(&'a IdeLock, &'a str), String> {
    let mut best: Option<(&IdeLock, &str)> = None;
    let mut multi_root = false;
    for lock in locks {
        for folder in &lock.workspace_folders {
            if !contains(folder, cwd) {
                continue;
            }
            if single_only && lock.workspace_folders.len() > 1 {
                multi_root = true;
                continue;
            }
            if best.is_none_or(|(_, f)| normalize(folder).len() > normalize(f).len()) {
                best = Some((lock, folder));
            }
        }
    }
    best.ok_or_else(|| {
        if multi_root {
            crate::i18n::t("The session's folder is part of a multi-root workspace, which can't be switched to.").into()
        } else {
            crate::i18n::t("No open VS Code window has this session's folder. Was the window closed?").into()
        }
    })
}

/// IDE lock files in `config_dir`. On Windows, locks of VS Code processes
/// that exited are skipped; WSL lock pids can't be checked from here.
pub fn read_locks(config_dir: &Path, check_alive: bool) -> Vec<IdeLock> {
    let Ok(entries) = std::fs::read_dir(config_dir.join("ide")) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("lock")))
        .filter_map(|e| {
            let lock = serde_json::from_slice::<IdeLock>(&std::fs::read(e.path()).ok()?).ok()?;
            // A lock left behind by a crash may name a pid another program has by now.
            let written = e.metadata().and_then(|m| m.modified()).ok();
            let written =
                written.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
            let alive = win::process_alive(lock.pid, None) && written.is_none_or(|w| win::started_by(lock.pid, w));
            (!check_alive || alive).then_some(lock)
        })
        .collect()
}

/// The workspace folder holding `cwd`, multi-root windows included.
pub fn workspace_folder(locks: &[IdeLock], cwd: &str) -> Option<String> {
    find_folder(locks, cwd, false).ok().map(|(_, f)| f.to_string())
}

/// Whether any lock names a folder holding `cwd`.
fn locked(locks: &[IdeLock], cwd: &str) -> bool {
    locks.iter().any(|l| l.workspace_folders.iter().any(|f| contains(f, cwd)))
}

/// `cwd`, or the nearest folder above it, that is open in a VS Code window:
/// VS Code has it among its windows' folders (`open`), and a window's title
/// names it (`name_of` gives a folder's window name). For when no lock names
/// the folder: Claude Code's extension sometimes loses its lock files while
/// the window stays open (seen when a window updates the extension while
/// others keep the old one). A title alone isn't enough: another window's
/// title can name a folder of the same name, or a file named like it.
fn folder_by_title(cwd: &str, titles: &[String], open: &[String], name_of: impl Fn(&Path) -> String) -> Option<String> {
    let mut dir = Some(Path::new(cwd));
    while let Some(d) = dir.filter(|d| d.file_name().is_some()) {
        let path = d.to_string_lossy();
        if open.iter().any(|f| normalize(f) == normalize(&path)) {
            let name = name_of(d);
            if titles.iter().any(|t| title_is(t, &name)) {
                return Some(path.into_owned());
            }
        }
        dir = d.parent();
    }
    None
}

/// The folders of VS Code's windows (Windows paths), as VS Code records them
/// in its `storage.json` to restore them. It keeps the record while it runs,
/// so a window just closed may still be in it: the window's title confirms it.
fn window_folders() -> Vec<String> {
    let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) else { return Vec::new() };
    let mut folders = Vec::new();
    for product in ["Code", "Code - Insiders"] {
        let file = appdata.join(product).join(r"User\globalStorage\storage.json");
        let Ok(v) = std::fs::read(&file).map(|b| serde_json::from_slice::<serde_json::Value>(&b).unwrap_or_default())
        else {
            continue;
        };
        let state = &v["windowsState"];
        let windows =
            state["openedWindows"].as_array().into_iter().flatten().chain(std::iter::once(&state["lastActiveWindow"]));
        folders.extend(windows.filter_map(|w| w["folder"].as_str()).filter_map(file_uri_path));
    }
    folders
}

/// `file:///c%3A/dev/app` → `c:\dev\app`; `None` for other URIs (remote windows).
fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file:///")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex =
            (bytes[i] == b'%').then(|| rest.get(i + 1..i + 3)).flatten().and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    Some(String::from_utf8(out).ok()?.replace('/', "\\"))
}

/// The folder of the open VS Code window holding `cwd` on Windows, found by
/// the window titles of VS Code's main process `pid`.
pub fn folder_by_window(pid: u32, cwd: &str, user_template: Option<&str>) -> Option<String> {
    folder_by_title(cwd, &win::window_titles(pid), &window_folders(), |d| window_name(d, user_template))
}

/// The folder of the open VS Code window holding `cwd` on Windows, from its
/// lock or, without one, from the window titles.
pub fn windows_folder(locks: &[IdeLock], cwd: &str, user_template: Option<&str>) -> Option<String> {
    if locked(locks, cwd) {
        return workspace_folder(locks, cwd);
    }
    folder_by_window(main_pid()?, cwd, user_template)
}

// ---------- window names ----------

/// The string value of `"key"` in a settings file. VS Code settings allow
/// comments and trailing commas, so this looks for the key instead of parsing
/// the whole file.
fn setting_string(text: &str, key: &str) -> Option<String> {
    let quoted = format!("\"{key}\"");
    // Every place the key stands: what's before it, and what follows.
    for (at, _) in text.match_indices(&quoted) {
        let (Some(before), Some(after)) = (text.get(..at), text.get(at + quoted.len()..)) else { continue };
        let line = before.rsplit_once('\n').map_or(before, |(_, line)| line);
        if line.contains("//") {
            continue; // commented out
        }
        let Some(value) = after.trim_start().strip_prefix(':').map(str::trim_start) else { continue };
        if !value.starts_with('"') {
            continue;
        }
        // Find the closing quote, skipping escaped characters.
        let bytes = value.as_bytes();
        let mut i = 1;
        while let Some(&b) = bytes.get(i).filter(|&&b| b != b'"') {
            i += if b == b'\\' { 2 } else { 1 };
        }
        return serde_json::from_str::<String>(value.get(..=i)?).ok();
    }
    None
}

/// The fixed start of a `window.title` template: text and the folder name,
/// up to the first part that changes with the open file. `"🚀 Shop | ${activeEditorShort}…"`
/// gives `🚀 Shop`; `"🧩 ${rootName}${separator}…"` gives `🧩 my-app`.
fn title_prefix(template: &str, root: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = template;
    loop {
        let Some((text, after)) = rest.split_once("${") else {
            out.push_str(rest);
            break;
        };
        out.push_str(text);
        let Some((variable, then)) = after.split_once('}') else { break };
        match variable {
            "rootName" | "rootNameShort" | "folderName" => out.push_str(root),
            _ => break,
        }
        rest = then;
    }
    let trimmed = out.trim().trim_end_matches(|c: char| c.is_whitespace() || "|-–—·:•".contains(c)).trim_end();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// VS Code's user-level `window.title`, if set.
pub fn user_window_title() -> Option<String> {
    let appdata = std::env::var_os("APPDATA")?;
    let text = std::fs::read_to_string(PathBuf::from(appdata).join(r"Code\User\settings.json")).ok()?;
    setting_string(&text, "window.title")
}

/// What VS Code calls the window of `folder` (a Windows path to it): the fixed
/// part of its `window.title`, or the folder name.
pub fn window_name(folder: &Path, user_template: Option<&str>) -> String {
    let root = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let workspace = std::fs::read_to_string(folder.join(r".vscode\settings.json"))
        .ok()
        .and_then(|t| setting_string(&t, "window.title"));
    workspace.as_deref().or(user_template).and_then(|t| title_prefix(t, &root)).unwrap_or(root)
}

// ---------- switching ----------

/// The URI scheme of this VS Code build (`vscode`, `vscode-insiders`, `cursor`…),
/// read from its `product.json`.
fn url_scheme(exe: &Path) -> String {
    url_protocol(exe).unwrap_or_else(|| "vscode".into())
}

/// The `urlProtocol` in the `product.json` next to `exe`: only VS Code and
/// the editors built on it have one.
fn url_protocol(exe: &Path) -> Option<String> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let mut candidates = vec![dir.join("resources\\app\\product.json")];
    // Newer builds keep resources in a per-version subfolder.
    if let Ok(entries) = std::fs::read_dir(dir) {
        candidates.extend(entries.flatten().map(|e| e.path().join("resources\\app\\product.json")));
    }
    candidates
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .find_map(|v| v["urlProtocol"].as_str().map(str::to_string))
}

/// The exe of the process `pid`, if it is VS Code (or an editor built on it).
/// The pid comes from a lock file or a hook and may be another program's by
/// now: nothing else is ever started with a session's folder.
fn vscode_exe(pid: u32) -> Result<PathBuf, String> {
    win::process_image(pid)
        .filter(|exe| url_protocol(exe).is_some())
        .ok_or_else(|| crate::i18n::t("Can't find VS Code's exe.").to_string())
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// `vscode-remote://wsl+<distro><folder>`, percent-encoding what URIs don't allow.
fn wsl_folder_uri(distro: &str, folder: &str) -> String {
    let encode = |s: &str, keep_slash: bool| -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
                b'/' if keep_slash => "/".into(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    format!("vscode-remote://wsl+{}{}", encode(distro, false), encode(folder, true))
}

/// VS Code's main process on Windows: a `Code.exe`-like process whose parent
/// isn't the same program.
fn main_process(table: &std::collections::HashMap<u32, win::ProcEntry>, exe_name: &str) -> Option<u32> {
    table
        .iter()
        .filter(|(_, e)| e.exe.eq_ignore_ascii_case(exe_name))
        .find(|(_, e)| table.get(&e.parent).is_none_or(|p| !p.exe.eq_ignore_ascii_case(exe_name)))
        .map(|(pid, _)| *pid)
}

/// The running VS Code's main process.
fn main_pid() -> Option<u32> {
    let table = win::process_table();
    main_process(&table, "Code.exe").or_else(|| main_process(&table, "Code - Insiders.exe"))
}

/// Path of the running VS Code's exe.
pub fn running_exe() -> Option<PathBuf> {
    win::process_image(main_pid()?)
}

/// Starts VS Code's exe like a second instance: it hands the arguments to the
/// running VS Code and exits. Environment variables that would redirect it
/// (e.g. when this app was started from a VS Code terminal) are cleared.
fn launch(exe: &Path, args: &[&str]) -> Result<Child, String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.creation_flags(CREATE_NO_WINDOW);
    for (key, _) in std::env::vars_os() {
        let k = key.to_string_lossy().to_uppercase();
        if k.starts_with("VSCODE_") || k.starts_with("ELECTRON_") {
            cmd.env_remove(&key);
        }
    }
    cmd.spawn().map_err(|e| crate::i18n::f("Couldn't start VS Code: {0}", &[&e]))
}

fn wait(mut child: Child, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
}

fn foreground_pid() -> u32 {
    let mut pid = 0u32;
    // SAFETY: plain queries with a valid out-pointer.
    unsafe {
        let hwnd = GetForegroundWindow();
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid
}

/// Whether a VS Code window title belongs to the window named `name` (the
/// start of a custom `window.title`, or the folder in VS Code's default
/// "file - folder - Visual Studio Code").
pub fn title_is(title: &str, name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && (title.strip_prefix(name).is_some_and(|rest| {
            // A whole name: not the start of a longer one.
            rest.chars().next().is_none_or(char::is_whitespace)
        }) || title.split(" - ").any(|p| p.split(" [").next().unwrap_or(p).trim() == name))
}

/// A finished session not looked at yet, as far as matching it to a window goes.
pub struct Unseen<'a> {
    pub id: &'a str,
    /// Its VS Code window's name, or the folder.
    pub window: &'a str,
    /// The conversation's title, if any.
    pub title: Option<&'a str>,
}

/// The finished sessions the VS Code window titled `title` shows, when that
/// is clear: the one whose conversation the title names (its tab is active),
/// or the only one in that window. A window with several gives none, since
/// which tab was looked at isn't known.
pub fn seen_in<'a>(title: &str, unseen: &[Unseen<'a>]) -> Vec<&'a str> {
    let here: Vec<&Unseen> = unseen.iter().filter(|u| title_is(title, u.window)).collect();
    let named: Vec<&str> = here
        .iter()
        .filter(|u| u.title.is_some_and(|t| !t.trim().is_empty() && title.contains(t.trim())))
        .map(|u| u.id)
        .collect();
    match (named.is_empty(), here.as_slice()) {
        (false, _) => named,
        (true, [only]) => vec![only.id],
        _ => Vec::new(),
    }
}

/// Switches to the session. Blocking; run it off the UI thread.
/// `config_dir` is Claude Code's config folder on Windows.
pub fn switch_to(config_dir: &Path, target: &Target) -> Result<(), String> {
    if !valid_session_id(&target.session_id) {
        return Err(crate::i18n::t("Unexpected session id.").into());
    }
    let (main_pid, exe, folder_arg): (u32, PathBuf, Vec<String>) = match &target.distro {
        None => {
            let locks: Vec<IdeLock> = read_locks(config_dir, true)
                .into_iter()
                .filter(|l| target.ide_pid.is_none_or(|p| p == l.pid))
                .collect();
            let (pid, folder) = if locked(&locks, &target.cwd) {
                let (lock, folder) = find_folder(&locks, &target.cwd, true)?;
                (lock.pid, folder.to_string())
            } else {
                // No lock: find the window by its title.
                let missing = find_folder(&locks, &target.cwd, true).err().unwrap_or_default();
                let pid = target.ide_pid.filter(|p| vscode_exe(*p).is_ok()).or_else(main_pid).ok_or(missing.clone())?;
                let folder = folder_by_window(pid, &target.cwd, user_window_title().as_deref()).ok_or(missing)?;
                (pid, folder)
            };
            (pid, vscode_exe(pid)?, vec![folder])
        }
        Some(distro) => {
            let locks: Vec<IdeLock> =
                registry::wsl_sources(distro).iter().flat_map(|s| read_locks(&s.config_dir, false)).collect();
            let (_, folder) = find_folder(&locks, &target.cwd, true)?;
            // WSL locks name the VS Code server inside Linux; the window belongs to VS Code on Windows.
            let table = win::process_table();
            let pid = main_process(&table, "Code.exe")
                .or_else(|| main_process(&table, "Code - Insiders.exe"))
                .ok_or(crate::i18n::t("VS Code isn't running."))?;
            (pid, vscode_exe(pid)?, vec!["--folder-uri".into(), wsl_folder_uri(distro, folder)])
        }
    };

    // We were just clicked, so we may hand the foreground on to VS Code.
    // SAFETY: plain call; failure only means VS Code may flash instead.
    let _ = unsafe { AllowSetForegroundWindow(main_pid) };
    let args: Vec<&str> = folder_arg.iter().map(String::as_str).collect();
    wait(launch(&exe, &args)?, Duration::from_secs(10));

    // Wait until a VS Code window is in front and give VS Code a moment to
    // register which window is active before handing it the URI.
    let start = Instant::now();
    while foreground_pid() != main_pid {
        if start.elapsed() > Duration::from_secs(3) {
            return Err(crate::i18n::t("VS Code didn't come to the front.").into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(350));

    let url = format!("{}://anthropic.claude-code/open?session={}", url_scheme(&exe), target.session_id);
    wait(launch(&exe, &["--open-url", "--", &url])?, Duration::from_secs(10));
    Ok(())
}

/// German translations of its messages (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("The session's folder is part of a multi-root workspace, which can't be switched to.", "Der Ordner der Sitzung gehört zu einem Arbeitsbereich mit mehreren Stammordnern; dorthin kann Trayward nicht wechseln."),
    ("No open VS Code window has this session's folder. Was the window closed?", "Kein offenes VS-Code-Fenster hat den Ordner dieser Sitzung. Wurde das Fenster geschlossen?"),
    ("Couldn't start VS Code: {0}", "VS Code ließ sich nicht starten: {0}"),
    ("Unexpected session id.", "Unerwartete Sitzungs-ID."),
    ("Can't find VS Code's exe.", "Die Programmdatei von VS Code wurde nicht gefunden."),
    ("VS Code isn't running.", "VS Code läuft nicht."),
    ("VS Code didn't come to the front.", "VS Code kam nicht nach vorne."),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(pid: u32, folders: &[&str]) -> IdeLock {
        IdeLock { pid, workspace_folders: folders.iter().map(std::string::ToString::to_string).collect() }
    }

    #[test]
    fn matches_window_titles() {
        assert!(title_is("📊 my-app | README.md (main)", "📊 my-app"));
        assert!(title_is("README.md - my-app - Visual Studio Code", "my-app"));
        assert!(title_is("notes.md - infrastructure [WSL: Ubuntu] (main)", "infrastructure"));
        assert!(!title_is("README.md - my-app-old - Visual Studio Code", "my-app"));
        assert!(!title_is("anything", " "));
        assert!(!title_is("📊 my-app-old | x (main)", "📊 my-app"));
    }

    #[test]
    fn finds_the_session_a_window_shows() {
        let u = |id, window, title| Unseen { id, window, title };
        let one = [u("a", "shop", Some("Fix the cart")), u("b", "api", None)];
        assert_eq!(seen_in("shop | cart.rs (main)", &one), ["a"]);
        assert!(seen_in("docs | x.md", &one).is_empty());
        // Two in one window: only the one its title names.
        let two = [u("a", "shop", Some("Fix the cart")), u("c", "shop", Some("Add a test"))];
        assert!(seen_in("shop | cart.rs (main)", &two).is_empty());
        assert_eq!(seen_in("shop | Fix the cart (main)", &two), ["a"]);
    }

    #[test]
    fn picks_deepest_single_folder_window() {
        let locks =
            [lock(1, &["c:\\dev"]), lock(2, &["C:/dev/tools/my-app/"]), lock(3, &["c:\\dev\\tools\\my-app-old"])];
        let (l, f) = find_folder(&locks, "c:\\dev\\tools\\my-app\\src", true).unwrap();
        assert_eq!((l.pid, f), (2, "C:/dev/tools/my-app/"));
        let (l, _) = find_folder(&locks, "c:\\dev\\other", true).unwrap();
        assert_eq!(l.pid, 1);
        let linux = [lock(4, &["/home/user/dev/infrastructure"])];
        assert!(find_folder(&linux, "/home/user/dev/infrastructure/terraform", true).is_ok());
    }

    #[test]
    fn rejects_prefix_lookalikes_and_multi_root() {
        let locks = [lock(1, &["c:\\dev\\app"])];
        assert!(find_folder(&locks, "c:\\dev\\application", true).is_err());
        let multi = [lock(1, &["c:\\a", "c:\\b"])];
        let err = find_folder(&multi, "c:\\a", true).unwrap_err();
        assert!(err.contains("multi-root"));
        assert_eq!(workspace_folder(&multi, "c:\\a\\x").as_deref(), Some("c:\\a"));
    }

    #[test]
    fn tells_whether_a_lock_names_the_folder() {
        let locks = [lock(1, &["c:\\dev\\app"]), lock(2, &["c:\\a", "c:\\b"])];
        assert!(locked(&locks, "c:\\dev\\app\\src"));
        assert!(locked(&locks, "C:/b"));
        assert!(!locked(&locks, "c:\\dev\\application"));
        assert!(!locked(&[], "c:\\dev\\app"));
    }

    #[test]
    fn finds_the_folder_by_window_title_without_a_lock() {
        let folder_name = |d: &Path| d.file_name().unwrap().to_string_lossy().into_owned();
        let titles = |t: &[&str]| t.iter().map(std::string::ToString::to_string).collect::<Vec<_>>();
        // VS Code's default title, and a custom one.
        let open = titles(&["main.rs - my-app - Visual Studio Code", "🚀 Shop | cart.rs (main)"]);
        let folders = titles(&[r"C:\dev\my-app", r"c:\web\shop", r"c:\dev"]);
        let find =
            |cwd: &str, t: &[String], name_of: &dyn Fn(&Path) -> String| folder_by_title(cwd, t, &folders, name_of);
        assert_eq!(find(r"c:\dev\my-app", &open, &folder_name).as_deref(), Some(r"c:\dev\my-app"));
        // A session started in a subfolder: the nearest open folder above it.
        assert_eq!(find(r"c:\dev\my-app\src\ui", &open, &folder_name).as_deref(), Some(r"c:\dev\my-app"));
        let custom = |d: &Path| if folder_name(d) == "shop" { "🚀 Shop".to_string() } else { folder_name(d) };
        assert_eq!(find(r"c:\web\shop", &open, &custom).as_deref(), Some(r"c:\web\shop"));
        // No window: none, and no similar names or drive roots.
        assert_eq!(find(r"c:\dev\my-app-old", &open, &folder_name), None);
        assert_eq!(find(r"c:\dev\other", &open, &folder_name), None);
        assert_eq!(find(r"c:\", &titles(&["c: - Visual Studio Code"]), &folder_name), None);
        assert_eq!(find(r"c:\dev\my-app", &[], &folder_name), None);
        // A window of another folder of the same name isn't this one: VS Code doesn't have this folder open.
        assert_eq!(find(r"x:\old\my-app", &open, &folder_name), None);
        // Nor is a window whose open file is named like the folder.
        let file = titles(&["dev.md - notes - Visual Studio Code"]);
        assert_eq!(find(r"c:\dev\tools", &file, &folder_name), None);
    }

    // cspell:ignore Cber Bubuntu
    #[test]
    fn reads_folders_of_file_uris() {
        assert_eq!(file_uri_path("file:///c%3A/dev/my%20app").as_deref(), Some(r"c:\dev\my app"));
        assert_eq!(file_uri_path("file:///c%3A/Users/u/%C3%BCber").as_deref(), Some(r"c:\Users\u\über"));
        assert_eq!(file_uri_path("vscode-remote://wsl%2Bubuntu/home/u"), None);
        assert_eq!(file_uri_path("file:///c%3A/bad%"), Some(r"c:\bad%".to_string()));
    }

    #[test]
    fn reads_window_title_setting() {
        let text = "{\n  // comment \"window.title\": \"no\"\n  \"editor.x\": 1,\n  \"window.title\": \"🚀 Shop | ${activeEditorShort} \\\"q\\\"\",\n}";
        assert_eq!(setting_string(text, "window.title").as_deref(), Some("🚀 Shop | ${activeEditorShort} \"q\""));
        assert_eq!(setting_string("{}", "window.title"), None);
    }

    #[test]
    fn takes_the_fixed_part_of_titles() {
        assert_eq!(
            title_prefix("🚀 Shop | ${activeEditorShort}${separator}${rootName}", "x").as_deref(),
            Some("🚀 Shop")
        );
        assert_eq!(
            title_prefix("🧩 ${rootName}${separator}${activeEditorShort}", "my-app").as_deref(),
            Some("🧩 my-app")
        );
        assert_eq!(title_prefix("${dirty}${activeEditorShort}${separator}${rootName}", "x"), None);
        assert_eq!(title_prefix("Fixed name", "x").as_deref(), Some("Fixed name"));
    }

    #[test]
    fn builds_wsl_uris() {
        assert_eq!(wsl_folder_uri("Ubuntu", "/home/user/my dev"), "vscode-remote://wsl+Ubuntu/home/user/my%20dev");
    }

    #[test]
    fn validates_session_ids() {
        assert!(valid_session_id("f8c204e9-e61a-4ee6-8037-3c078f9b9953"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("abc&prompt=rm"));
    }

    /// A folder of its own in the temp folder, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!("trayward-vscode-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A pid no process has (Windows' pids are multiples of 4).
    const NO_PROCESS: u32 = u32::MAX - 2;

    #[test]
    fn reads_the_lock_files() {
        let config = TempDir::new("locks");
        let ide = config.0.join("ide");
        std::fs::create_dir_all(&ide).unwrap();
        let me = std::process::id();
        let write = |name: &str, text: String| std::fs::write(ide.join(name), text).unwrap();
        write("1.lock", format!(r#"{{"pid":{me},"workspaceFolders":["c:\\dev\\app"],"ideName":"VS Code"}}"#));
        write("2.LOCK", format!(r#"{{"pid":{NO_PROCESS},"workspaceFolders":["c:\\dev\\gone"]}}"#));
        // Without folders, broken, or not a lock file: as far as it can be read.
        write("3.lock", format!(r#"{{"pid":{me}}}"#));
        write("4.lock", "{not json".into());
        write("5.json", format!(r#"{{"pid":{me},"workspaceFolders":["c:\\x"]}}"#));
        let pids = |locks: &[IdeLock]| {
            let mut pids: Vec<u32> = locks.iter().map(|l| l.pid).collect();
            pids.sort_unstable();
            pids
        };
        // Every lock (WSL's pids can't be checked), or those of processes that run.
        assert_eq!(pids(&read_locks(&config.0, false)), [me, me, NO_PROCESS]);
        let alive = read_locks(&config.0, true);
        assert_eq!(pids(&alive), [me, me]);
        assert!(alive.iter().any(|l| l.workspace_folders.is_empty()));
        // A lock names the folder: its window's folder, from the lock.
        assert_eq!(windows_folder(&alive, r"c:\dev\app\src", None).as_deref(), Some(r"c:\dev\app"));
        // No lock folder at all.
        assert!(read_locks(&config.0.join("nowhere"), false).is_empty());
    }

    #[test]
    fn names_a_window_after_its_title_setting() {
        let dir = TempDir::new("names");
        let folder = dir.0.join("my-app");
        std::fs::create_dir_all(folder.join(".vscode")).unwrap();
        // No setting: the folder's name, or the start of the user's template.
        assert_eq!(window_name(&folder, None), "my-app");
        assert_eq!(window_name(&folder, Some("🧩 ${rootName}${separator}${activeEditorShort}")), "🧩 my-app");
        assert_eq!(window_name(&folder, Some("${activeEditorShort}")), "my-app");
        // The workspace's own setting wins.
        std::fs::write(
            folder.join(r".vscode\settings.json"),
            r#"{ "window.title": "🚀 Shop | ${activeEditorShort}", }"#,
        )
        .unwrap();
        assert_eq!(window_name(&folder, Some("${rootName}")), "🚀 Shop");
        // A path without a folder's name.
        assert_eq!(window_name(&folder.join(".."), None), "");
    }

    #[test]
    fn reads_the_url_scheme_of_an_editor() {
        let dir = TempDir::new("product");
        let exe = dir.0.join("Code.exe");
        // None: VS Code's own.
        assert_eq!(url_protocol(&exe), None);
        assert_eq!(url_scheme(&exe), "vscode");
        // Newer builds keep it in a folder per version.
        let versioned = dir.0.join(r"1a2b3c\resources\app");
        std::fs::create_dir_all(&versioned).unwrap();
        std::fs::write(versioned.join("product.json"), r#"{"nameShort":"Code","urlProtocol":"vscode-insiders"}"#)
            .unwrap();
        assert_eq!(url_scheme(&exe), "vscode-insiders");
        let direct = dir.0.join(r"resources\app");
        std::fs::create_dir_all(&direct).unwrap();
        std::fs::write(direct.join("product.json"), r#"{"urlProtocol":"cursor"}"#).unwrap();
        assert_eq!(url_scheme(&exe), "cursor");
        // This test isn't VS Code; a process that isn't there neither.
        assert!(vscode_exe(std::process::id()).is_err());
        assert!(vscode_exe(NO_PROCESS).is_err());
    }

    #[test]
    fn finds_the_main_process_of_vs_code() {
        let entry = |parent, exe: &str| win::ProcEntry { parent, exe: exe.into() };
        let table: std::collections::HashMap<u32, win::ProcEntry> = [
            (1, entry(0, "explorer.exe")),
            (7, entry(1, "Code.exe")),
            (8, entry(7, "Code.exe")),
            (9, entry(8, "code.EXE")),
            (20, entry(1, "Code - Insiders.exe")),
        ]
        .into_iter()
        .collect();
        assert_eq!(main_process(&table, "Code.exe"), Some(7));
        assert_eq!(main_process(&table, "Code - Insiders.exe"), Some(20));
        assert_eq!(main_process(&table, "Cursor.exe"), None);
        // A parent that's gone: it's the main one.
        let orphan = [(9, entry(5, "Code.exe"))].into_iter().collect();
        assert_eq!(main_process(&orphan, "Code.exe"), Some(9));
    }

    #[test]
    fn switches_only_to_a_session_id() {
        let target = Target { session_id: "x&y".into(), cwd: r"c:\dev".into(), ide_pid: None, distro: None };
        assert!(switch_to(Path::new("nowhere"), &target).unwrap_err().contains("session id"));
        // The window in front belongs to some process (none in a session without a desktop).
        let _ = foreground_pid();
        // A VS Code that runs is found by its processes (only looked up).
        assert!(running_exe().is_none_or(|exe| exe.is_absolute()));
    }

    #[test]
    fn says_why_no_window_was_found() {
        crate::i18n::set(crate::i18n::Choice::English);
        let none = find_folder(&[], r"c:\dev", false).unwrap_err();
        assert!(none.contains("Was the window closed?"), "{none}");
        assert_eq!(workspace_folder(&[], r"c:\dev"), None);
    }
}
