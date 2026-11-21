//! Reads Claude Code's own list of running sessions: one file per Claude Code
//! process in `<config>/sessions/<pid>.json` with its session id, folder and
//! state (`busy`, `waiting`, `idle`).
//!
//! Sessions run on Windows (`~/.claude`) or inside WSL distributions
//! (`\\wsl.localhost\<distro>\home\<user>\.claude`). Only distributions that
//! are already running are read: touching a stopped one would start it, and
//! touching one nobody uses keeps it from stopping (the caller spaces those
//! looks out).
//!
//! The files are internal to Claude Code and undocumented, so everything here
//! is best effort: anything unexpected is skipped, never an error.

use crate::os;
use crate::sessions::{Listed, ListedState};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Where sessions are read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// WSL distribution, or `None` for Windows.
    pub distro: Option<String>,
    /// Claude Code's config folder, as a Windows path.
    pub config_dir: PathBuf,
}

impl Source {
    /// A WSL distribution that isn't running: its files can't be read without
    /// starting it, so only what was kept of its sessions counts (`keep`). Its
    /// folder is one no file system has (a name with a NUL in it), so nothing
    /// that reads a source's files finds any here.
    pub fn kept_only(distro: &str) -> Source {
        Source { distro: Some(distro.to_string()), config_dir: PathBuf::from("\0") }
    }
}

/// Parses one session file. `distro` is where it came from.
fn parse(v: &Value, distro: Option<&str>) -> Option<Listed> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    if v["spare"] == Value::Bool(true) || v["kind"].as_str().is_some_and(|k| k != "interactive") {
        return None;
    }
    Some(Listed {
        id: text("sessionId").filter(|id| crate::hook::valid_session_id(id))?,
        pid: u32::try_from(v["pid"].as_u64()?).ok()?,
        // Windows: process creation time (FILETIME ticks). Linux: start time in clock ticks.
        started: v["procStart"].as_str().and_then(|s| s.parse().ok()),
        cwd: text("cwd").unwrap_or_default(),
        name: text("name"),
        state: ListedState::parse(v["status"].as_str().unwrap_or_default()),
        state_at_ms: v["statusUpdatedAt"].as_i64().or_else(|| v["updatedAt"].as_i64()).unwrap_or(0),
        entrypoint: text("entrypoint"),
        started_ms: v["startedAt"].as_i64(),
        distro: distro.map(str::to_string),
    })
}

fn read_dir_json(dir: &Path) -> Option<Vec<Value>> {
    let entries = std::fs::read_dir(dir).ok()?;
    Some(
        entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .collect(),
    )
}

/// Running sessions of `source`, or `None` if its folder couldn't be read.
/// Entries whose process has exited, or whose pid now belongs to another
/// process, are left out, as are spare processes Claude Code starts ahead.
pub fn list(source: &Source) -> Option<Vec<Listed>> {
    let values = read_dir_json(&source.config_dir.join("sessions"))?;
    let distro = source.distro.as_deref();
    Some(
        values
            .iter()
            .filter_map(|v| parse(v, distro))
            .filter(|l| match distro {
                None => os::process_alive(l.pid, l.started),
                Some(d) => linux_process_alive(d, l.pid, l.started),
            })
            .collect(),
    )
}

/// `\\wsl.localhost\<distro>` plus a Linux path.
pub fn wsl_path(distro: &str, linux: &str) -> PathBuf {
    PathBuf::from(format!(r"\\wsl.localhost\{distro}{}", linux.replace('/', "\\")))
}

/// Start time (clock ticks after boot) of a Linux process: field 22 of
/// `/proc/<pid>/stat`, counted after the parenthesised command name.
fn linux_start_time(stat: &str) -> Option<u64> {
    let (_, after) = stat.rsplit_once(')')?;
    after.split_whitespace().nth(19)?.parse().ok()
}

fn linux_process_alive(distro: &str, pid: u32, started: Option<u64>) -> bool {
    let Ok(stat) = std::fs::read_to_string(wsl_path(distro, &format!("/proc/{pid}/stat"))) else { return false };
    match started {
        Some(s) => linux_start_time(&stat) == Some(s),
        None => true,
    }
}

/// Names of the WSL distributions that are running right now; `None` when
/// `wsl.exe` couldn't be asked (missing, slow, failing). "None running" ends
/// with exit code 1 and no list, which counts as an answer.
pub fn wsl_running() -> Option<Vec<String>> {
    let args = ["--list", "--running", "--quiet"];
    let exe = crate::os::system_program("wsl.exe")?;
    let out = crate::os::run(exe.as_os_str(), &args, std::time::Duration::from_secs(10)).ok()?;
    out.complete.then(|| parse_wsl_list(&out.stdout))
}

/// `wsl.exe` writes UTF-16; Docker Desktop's internal distributions are skipped.
fn parse_wsl_list(bytes: &[u8]) -> Vec<String> {
    let utf16: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&utf16)
        .lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0'))
        .filter(|l| !l.is_empty() && !l.starts_with("docker-desktop"))
        .map(str::to_string)
        .collect()
}

/// Claude Code config folders inside a running distribution: `~/.claude` of
/// every user in `/home`, and root's.
pub fn wsl_sources(distro: &str) -> Vec<Source> {
    let mut homes: Vec<PathBuf> = std::fs::read_dir(wsl_path(distro, "/home"))
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    homes.push(wsl_path(distro, "/root"));
    homes
        .into_iter()
        .map(|h| h.join(".claude"))
        .filter(|c| c.join("sessions").is_dir())
        .map(|config_dir| Source { distro: Some(distro.to_string()), config_dir })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_session_files() {
        let v = json!({
            "pid": 54252, "sessionId": "f8c2", "cwd": "c:\\dev\\x", "procStart": "134389051362827638",
            "kind": "interactive", "entrypoint": "claude-vscode", "name": "x-ae",
            "status": "waiting", "waitingFor": "permission", "statusUpdatedAt": 1794431736725_i64
        });
        let l = parse(&v, None).unwrap();
        assert_eq!(l.pid, 54252);
        assert_eq!(l.started, Some(134_389_051_362_827_638));
        assert_eq!(l.state, ListedState::Waiting);
        assert_eq!(l.state_at_ms, 1_794_431_736_725);
        assert_eq!(l.name.as_deref(), Some("x-ae"));
        assert_eq!(parse(&v, Some("Ubuntu")).unwrap().distro.as_deref(), Some("Ubuntu"));
    }

    #[test]
    fn skips_spares_and_non_interactive() {
        let base = json!({"pid": 1, "sessionId": "a", "kind": "interactive", "status": "idle"});
        assert!(parse(&base, None).is_some());
        let mut spare = base.clone();
        spare["spare"] = json!(true);
        assert!(parse(&spare, None).is_none());
        let mut print = base.clone();
        print["kind"] = json!("print");
        assert!(parse(&print, None).is_none());
        assert!(parse(&json!({"pid": 1}), None).is_none());
        // An id that is a path isn't a session's.
        let mut path = base.clone();
        path["sessionId"] = json!(r"\\host\share\x");
        assert!(parse(&path, None).is_none());
    }

    #[test]
    fn reads_linux_start_time() {
        let stat = "1924011 (claude (x) y) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 48964645 99 100";
        assert_eq!(linux_start_time(stat), Some(48_964_645));
        assert_eq!(linux_start_time("garbage"), None);
    }

    #[test]
    fn parses_wsl_list_output() {
        let text = "\u{feff}Ubuntu\r\ndocker-desktop\r\nDebian\r\n";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(parse_wsl_list(&bytes), ["Ubuntu", "Debian"]);
        assert!(parse_wsl_list(&[]).is_empty());
    }

    #[test]
    fn lists_the_sessions_that_run() {
        let config = std::env::temp_dir().join(format!("trayward-registry-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&config);
        let sessions = config.join("sessions");
        let source = Source { distro: None, config_dir: config.clone() };
        // No folder: nothing could be read.
        assert_eq!(list(&source), None);
        std::fs::create_dir_all(&sessions).unwrap();
        let entry = |pid: u32, id: &str| {
            json!({"pid": pid, "sessionId": id, "cwd": "C:\\work\\shop", "kind": "interactive", "status": "busy"})
                .to_string()
        };
        // This process runs; a pid past any there can be doesn't.
        std::fs::write(sessions.join("1.json"), entry(std::process::id(), "running")).unwrap();
        std::fs::write(sessions.join("2.JSON"), entry(u32::MAX - 1, "ended")).unwrap();
        // Not JSON, not a session list entry, not a .json file.
        std::fs::write(sessions.join("3.json"), "{ cut off").unwrap();
        std::fs::write(sessions.join("4.json"), "[1, 2]").unwrap();
        std::fs::write(sessions.join("notes.txt"), entry(std::process::id(), "text")).unwrap();
        let listed = list(&source).unwrap();
        assert_eq!(listed.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["running"]);
        assert_eq!((listed[0].state, listed[0].distro.as_deref()), (ListedState::Busy, None));
        // A distribution that isn't running has no folder to read.
        assert_eq!(list(&Source::kept_only("Ubuntu")), None);
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn builds_wsl_paths() {
        assert_eq!(
            wsl_path("Ubuntu", "/home/user/.claude"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\user\.claude")
        );
    }
}
