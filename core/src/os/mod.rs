//! What Trayward asks of the operating system: processes, running other
//! programs, local time, the display language, changes in a folder, HTTPS and TLS. The logic in
//! this crate goes through here and never calls a system API itself, so
//! another system only needs its own module next to `windows`.
//!
//! - `windows/`: Windows (Win32, WinHTTP, SChannel). The one platform today.
//! - `other.rs`: any other system, for now with plain fallbacks (UTC time, no
//!   process details, no HTTPS), so this crate builds and its tests run there.
//!
//! What is the same everywhere (the clock, running a program, finding one on
//! PATH) is in this file.

use std::path::PathBuf;

#[cfg(windows)]
#[path = "windows/mod.rs"]
mod platform;

#[cfg(not(windows))]
#[path = "other.rs"]
mod platform;

use platform::current_path;
pub use platform::{
    DirWatch, background_thread, http, local_date, local_time, process_alive, process_image, process_start_time,
    process_table, secret, started_by, system_language_is_german, tls, trim_heap, utc_offset,
};

/// A process in [`process_table`].
pub struct ProcEntry {
    pub parent: u32,
    /// Executable file name, e.g. `claude.exe`.
    pub exe: String,
}

/// What an HTTPS request got back (see [`http`]).
pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
    /// The `Retry-After` header in seconds, when the server sent one as a number.
    pub retry_after: Option<u32>,
}

/// What a program's name may end in, in the order they're tried.
#[cfg(windows)]
const PROGRAM_EXTENSIONS: &[&str] = &[".exe", ".cmd", ".bat"];
#[cfg(not(windows))]
const PROGRAM_EXTENSIONS: &[&str] = &[""];

/// Current time as unix milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
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
    run_env(program, args, &[], timeout)
}

/// [`run`] with environment variables of its own on top of Trayward's (a token
/// for `glab` or `gh`: never on the command line, which other programs can read).
pub fn run_env(
    program: &std::ffi::OsStr,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: std::time::Duration,
) -> Result<Output, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    let name = std::path::Path::new(program).file_stem().unwrap_or(program).to_string_lossy().into_owned();
    let mut command = Command::new(program);
    command.args(args).envs(env.iter().copied()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| format!("{name}: {e}"))?;
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
                    let read = chunk.get(..n).unwrap_or_default();
                    got.lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend_from_slice(read);
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
    let take = |(got, _): &Pipe| std::mem::take(&mut *got.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
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
/// the folders in PATH: the one Trayward started with, then PATH as it is now
/// (a tool installed since is found without restarting Trayward). Windows would
/// look in the folder of Trayward's exe (often Downloads) first, and in the
/// working folder: not here.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let own = std::env::current_exe().ok().and_then(|e| e.parent().map(std::path::Path::to_path_buf));
    let started: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    let find = |dirs: &mut dyn Iterator<Item = PathBuf>| {
        dirs.filter(|dir| dir.is_absolute() && own.as_deref() != Some(dir.as_path()))
            .flat_map(|dir| PROGRAM_EXTENSIONS.iter().map(move |ext| dir.join(format!("{name}{ext}"))))
            .find(|p| p.is_file())
    };
    // The registry's PATH only when the one Trayward started with has no match,
    // and only its folders that weren't searched already.
    find(&mut started.iter().cloned()).or_else(|| {
        let now = current_path()?;
        find(&mut std::env::split_paths(&now).filter(|dir| !started.contains(dir)))
    })
}
