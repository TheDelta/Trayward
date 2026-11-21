//! OpenAI's Codex CLI as a second provider: the limits of the ChatGPT plan it
//! runs on (a 5-hour and a weekly window) and its sessions. Both come from the
//! session files Codex writes, `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`
//! (`CODEX_HOME` moves `~/.codex`): no network, nothing written.
//!
//! Each line is `{"timestamp", "type", "payload"}`. Used here:
//! - `session_meta`: the session's id and folder (first line);
//! - `turn_context`: the model;
//! - `event_msg` `user_message` / `agent_message`: prompts and replies;
//! - `event_msg` `token_count`: tokens and `rate_limits` (`primary` / `secondary`
//!   with `used_percent`, `window_minutes`, `resets_at`, older versions
//!   `resets_in_seconds`; `plan_type`);
//! - `response_item` `function_call` / `function_call_output`: tool calls.
//!
//! Session files can grow to hundreds of megabytes, so only the start (for
//! the id, folder and first prompt) and the end (for the latest state) are read.
//! Codex doesn't record that it waits for an approval: a tool call without a
//! result for a while counts as needing the user.

use crate::sessions::Status;
use crate::timefmt::parse_rfc3339;
use crate::usage::Limit;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// What's read from the start and the end of a session file.
const HEAD_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 256 * 1024;
/// Limits are taken from session files of the last week.
const LIMITS_DAYS: i64 = 8;
/// How often every day's folder is looked at, however old: a session that goes on
/// for longer than that (left open, or resumed) is in the folder of its first day.
const WALK_EVERY_SECS: i64 = 10 * 60;
/// Sessions are listed when their file changed within a day.
const LIST_SECS: i64 = 24 * 3600;
/// Written to this recently: working.
const WORKING_SECS: i64 = 15;
/// A tool call without a result for this long: waiting for an approval.
const APPROVAL_SECS: i64 = 20;
/// Done this long ago: idle.
const IDLE_AFTER_SECS: i64 = 3600;

/// Codex's limits, as of its last use.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub five_hour: Option<Limit>,
    pub weekly: Option<Limit>,
    /// When Codex reported them (unix seconds).
    pub at: i64,
}

/// A Codex session (Copilot's sessions use it too).
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    /// The first prompt.
    pub title: String,
    pub status: Status,
    /// Since when it has had this status (unix seconds).
    pub since: i64,
    pub model: Option<String>,
    /// Tokens in the context window after the last step.
    pub context_tokens: u64,
}

impl Session {
    /// Folder name of the working directory.
    pub fn project(&self) -> &str {
        let trimmed = self.cwd.trim_end_matches(['\\', '/']);
        trimmed.rsplit(['\\', '/']).next().filter(|s| !s.is_empty()).unwrap_or("—")
    }
}

/// Everything read from Codex's files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub limits: Option<Limits>,
    /// The plan, e.g. "Plus" or "Pro".
    pub plan: Option<String>,
    pub sessions: Vec<Session>,
}

/// Codex's folder, if Codex is used on this PC (the demo's own one in demo mode).
pub fn home() -> Option<PathBuf> {
    let dir = if crate::demo::active() {
        crate::demo::codex_dir()?
    } else if let Some(h) = std::env::var_os("CODEX_HOME").filter(|h| !h.is_empty()) {
        PathBuf::from(h)
    } else {
        PathBuf::from(std::env::var_os("USERPROFILE")?).join(".codex")
    };
    dir.join("sessions").is_dir().then_some(dir)
}

/// What a session file says, as far as Trayward needs it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Parsed {
    id: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    model: Option<String>,
    /// Latest rate limits: when, the limits, the plan.
    limits: Option<(i64, Limits, Option<String>)>,
    last_prompt: Option<i64>,
    last_reply: Option<i64>,
    /// A tool call started (unix seconds) that has no result yet.
    open_call: Option<i64>,
    context_tokens: u64,
}

/// Reads Codex's session files, re-reading only those that changed.
#[derive(Default)]
pub struct Reader {
    cache: HashMap<PathBuf, (u64, i64, Parsed)>,
    /// When every day's folder was last looked at (unix seconds).
    walked: Option<i64>,
    /// The day folders that had a file in use then or since.
    live_days: std::collections::HashSet<PathBuf>,
}

impl Reader {
    pub fn read(&mut self, home: &Path, now: i64) -> Snapshot {
        let all = self.walked.is_none_or(|t| !(0..WALK_EVERY_SECS).contains(&(now - t)));
        let live = &self.live_days;
        let files = recent_files(&home.join("sessions"), now - LIMITS_DAYS * 86_400, |day| all || live.contains(day));
        if all {
            self.walked = Some(now);
        }
        self.live_days = files.iter().filter_map(|(f, _, _)| f.parent().map(Path::to_path_buf)).collect();
        self.cache.retain(|p, _| files.iter().any(|(f, _, _)| f == p));
        let mut snap = Snapshot::default();
        for (path, len, mtime) in files {
            let parsed = match self.cache.get(&path) {
                Some((l, m, p)) if *l == len && *m == mtime => p.clone(),
                _ => {
                    let p = parse_file(&path, len);
                    self.cache.insert(path.clone(), (len, mtime, p.clone()));
                    p
                }
            };
            if let Some((at, limits, plan)) = &parsed.limits {
                if snap.limits.is_none_or(|l| l.at < *at) {
                    snap.limits = Some(Limits { at: *at, ..*limits });
                    snap.plan.clone_from(plan);
                }
            }
            if now - mtime <= LIST_SECS {
                if let Some(s) = session(&path, &parsed, mtime, now) {
                    snap.sessions.push(s);
                }
            }
        }
        snap.sessions.sort_by_key(|s| std::cmp::Reverse(s.since));
        snap
    }
}

/// Session files changed since `since`: path, length and modification time.
/// `look` names the day folders to look into whatever their own age.
fn recent_files(sessions: &Path, since: i64, look: impl Fn(&Path) -> bool) -> Vec<(PathBuf, u64, i64)> {
    let mut out = Vec::new();
    // YYYY/MM/DD: a day's folder changes when a file is added to it, not when one
    // in it grows. Older days are skipped without looking at their files, except
    // the ones `look` names.
    for year in subfolders(sessions) {
        for month in subfolders(&year) {
            for day in subfolders(&month) {
                let Ok(meta) = std::fs::metadata(&day) else { continue };
                if mtime(&meta) >= since - 86_400 || look(&day) {
                    day_files(&day, since, &mut out);
                }
            }
        }
    }
    out
}

/// When a file or folder last changed, as unix seconds (0 if unknown).
fn mtime(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// The folders in `p`.
fn subfolders(p: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(p)
        .map(|r| r.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// A day folder's session files changed since `since`, added to `out`.
fn day_files(day: &Path, since: i64, out: &mut Vec<(PathBuf, u64, i64)>) {
    for e in std::fs::read_dir(day).into_iter().flatten().flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "jsonl") {
            continue;
        }
        if let Some(m) = e.metadata().ok().filter(|m| mtime(m) >= since) {
            out.push((path, m.len(), mtime(&m)));
        }
    }
}

fn parse_file(path: &Path, len: u64) -> Parsed {
    let mut p = Parsed::default();
    let Ok(mut f) = std::fs::File::open(path) else { return p };
    let mut head = Vec::new();
    let _ = (&mut f).take(HEAD_BYTES.min(len)).read_to_end(&mut head);
    // The start: the session's id, folder and first prompt.
    for line in complete_lines(&head, false) {
        parse_line(&mut p, line, true);
    }
    // The end: the latest state. (A short file is read whole above.)
    if len > HEAD_BYTES {
        let far = len.saturating_sub(TAIL_BYTES);
        // Reaching back to the start: on from the start's last whole line, so the
        // line across the border is read too. Else from inside some line.
        let head_end = head.iter().rposition(|&b| b == b'\n').map_or(0, |i| i as u64 + 1);
        let (start, cut) = if far <= HEAD_BYTES { (head_end, false) } else { (far, true) };
        let mut tail = Vec::new();
        if f.seek(SeekFrom::Start(start)).is_ok() {
            let _ = f.take(len - start).read_to_end(&mut tail);
        }
        for line in complete_lines(&tail, cut) {
            parse_line(&mut p, line, false);
        }
    }
    p
}

/// The whole lines in `buf`; a cut-off first line (`skip_first`) and an
/// unfinished last line are left out.
fn complete_lines(buf: &[u8], skip_first: bool) -> impl Iterator<Item = &str> {
    let end = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let whole = buf.get(..end).unwrap_or_default();
    // Cut at the bytes, not the text: a read can start inside a character,
    // which would make the whole buffer invalid UTF-8.
    let start = if skip_first { whole.iter().position(|&b| b == b'\n').map_or(end, |i| i + 1) } else { 0 };
    let lines = whole.get(start..).unwrap_or_default();
    lines.split(|&b| b == b'\n').filter_map(|l| std::str::from_utf8(l).ok()).filter(|l| !l.trim().is_empty())
}

fn parse_line(p: &mut Parsed, line: &str, head: bool) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let at = v["timestamp"].as_str().and_then(parse_rfc3339).unwrap_or(0);
    let payload = &v["payload"];
    let text = |key: &str| payload[key].as_str().map(str::to_string);
    match (v["type"].as_str(), payload["type"].as_str()) {
        (Some("session_meta"), _) => {
            p.id = p.id.take().or_else(|| text("id"));
            p.cwd = p.cwd.take().or_else(|| text("cwd"));
        }
        (Some("turn_context"), _) => {
            p.model = text("model").or(p.model.take());
            if p.cwd.is_none() {
                p.cwd = text("cwd");
            }
        }
        (Some("event_msg"), Some("user_message")) => {
            if head && p.title.is_none() {
                p.title = text("message").map(|m| first_line(&m));
            }
            p.last_prompt = Some(at);
            p.open_call = None;
        }
        (Some("event_msg"), Some("agent_message")) => {
            p.last_reply = Some(at);
            p.open_call = None;
        }
        (Some("event_msg"), Some("turn_aborted")) => {
            p.last_reply = Some(at);
            p.open_call = None;
        }
        (Some("event_msg"), Some("token_count")) => {
            if let Some(t) = payload["info"]["last_token_usage"]["input_tokens"].as_u64() {
                p.context_tokens = t;
            }
            if let Some(l) = rate_limits(&payload["rate_limits"], at) {
                let plan = payload["rate_limits"]["plan_type"].as_str().filter(|s| !s.is_empty()).map(plan_name);
                p.limits = Some((at, l, plan));
            }
        }
        (Some("response_item"), Some("function_call" | "custom_tool_call" | "local_shell_call")) => {
            p.open_call = Some(at);
        }
        (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => p.open_call = None,
        (Some("response_item"), Some("message")) if payload["role"] == "assistant" => {
            p.last_reply = Some(at);
            p.open_call = None;
        }
        _ => {}
    }
}

/// The 5-hour and the weekly window from a `rate_limits` object.
fn rate_limits(v: &Value, at: i64) -> Option<Limits> {
    let mut limits = Limits::default();
    for (key, fallback_weekly) in [("primary", false), ("secondary", true)] {
        let w = &v[key];
        let Some(used) = w["used_percent"].as_f64() else { continue };
        let resets_at =
            w["resets_at"].as_i64().or_else(|| w["resets_in_seconds"].as_i64().map(|s| at + s)).filter(|t| *t > 0);
        let limit = Limit { used: used.clamp(0.0, 100.0), resets_at };
        // The window's length says which one it is; else primary is the 5-hour one.
        let weekly = w["window_minutes"].as_i64().map_or(fallback_weekly, |m| m > 10 * 60);
        if weekly {
            limits.weekly = Some(limit);
        } else {
            limits.five_hour = Some(limit);
        }
    }
    (limits.weekly.is_some() || limits.five_hour.is_some()).then_some(limits)
}

/// "plus" → "Plus".
pub(crate) fn plan_name(t: &str) -> String {
    let mut c = t.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

pub(crate) fn first_line(s: &str) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let mut out: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        out.push('…');
    }
    out
}

fn session(path: &Path, p: &Parsed, mtime: i64, now: i64) -> Option<Session> {
    // The id is the session's; else the file name's end (a UUID).
    let id = p.id.clone().or_else(|| {
        let stem = path.file_stem()?.to_string_lossy().to_string();
        Some(stem.get(stem.len().saturating_sub(36)..)?.to_string())
    })?;
    let idle = now - mtime;
    let replied = p.last_reply.is_some_and(|r| p.last_prompt.is_none_or(|q| r >= q));
    let (status, since) = if idle < WORKING_SECS && p.last_prompt.is_some() {
        (Status::Working, p.last_prompt.unwrap_or(mtime))
    } else if let Some(call) = p.open_call.filter(|_| (APPROVAL_SECS..IDLE_AFTER_SECS).contains(&idle)) {
        (Status::NeedsInput, call.max(mtime))
    } else if replied && idle < IDLE_AFTER_SECS {
        (Status::Done, mtime)
    } else {
        (Status::Idle, mtime)
    };
    Some(Session {
        id: format!("codex:{id}"),
        cwd: p.cwd.clone().unwrap_or_default(),
        title: p.title.clone().unwrap_or_default(),
        status,
        since,
        model: p.model.clone(),
        context_tokens: p.context_tokens,
    })
}

/// Reads Codex's files every few seconds on its own thread and posts a
/// [`Snapshot`] to `send` when something changed. Looks only while `on`.
pub fn spawn(send: impl Fn(Snapshot) + Send + 'static, on: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    std::thread::spawn(move || {
        crate::os::background_thread();
        let mut reader = Reader::default();
        let mut last: Option<Snapshot> = None;
        loop {
            let snap = if on.load(std::sync::atomic::Ordering::Relaxed) {
                home().map(|h| reader.read(&h, crate::os::now_ms() / 1000))
            } else {
                None
            };
            let snap = snap.unwrap_or_default();
            if last.as_ref() != Some(&snap) {
                last = Some(snap.clone());
                send(snap);
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = r#"{"timestamp":"2026-11-12T10:00:00.000Z","type":"session_meta","payload":{"id":"0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b","timestamp":"2026-11-12T10:00:00.000Z","cwd":"C:\\work\\shop","originator":"codex_cli_rs","cli_version":"0.63.0"}}"#;

    fn line(at: &str, kind: &str, payload: &str) -> String {
        format!(r#"{{"timestamp":"2026-11-12T{at}.000Z","type":"{kind}","payload":{payload}}}"#)
    }

    fn parse(lines: &[String]) -> Parsed {
        let mut p = Parsed::default();
        let mut all = vec![META.to_string()];
        all.extend_from_slice(lines);
        for l in &all {
            parse_line(&mut p, l, true);
        }
        p
    }

    #[test]
    fn reads_limits_and_plan() {
        let p = parse(&[line(
            "10:05:00",
            "event_msg",
            r#"{"type":"token_count","info":{"last_token_usage":{"input_tokens":51234}},"rate_limits":{"primary":{"used_percent":12.5,"window_minutes":300,"resets_at":1794500000},"secondary":{"used_percent":30.0,"window_minutes":10080,"resets_in_seconds":3600},"plan_type":"plus"}}"#,
        )]);
        let (at, l, plan) = p.limits.unwrap();
        assert_eq!(l.five_hour.unwrap().used, 12.5);
        assert_eq!(l.five_hour.unwrap().resets_at, Some(1_794_500_000));
        // Older versions: seconds from the record's time.
        assert_eq!(l.weekly.unwrap().resets_at, Some(at + 3600));
        assert_eq!(plan.as_deref(), Some("Plus"));
        assert_eq!(p.context_tokens, 51_234);
        assert_eq!(p.cwd.as_deref(), Some("C:\\work\\shop"));
    }

    #[test]
    fn tells_the_status() {
        let prompt = line("10:01:00", "event_msg", r#"{"type":"user_message","message":"Fix the login test\nmore"}"#);
        let call = line(
            "10:01:05",
            "response_item",
            r#"{"type":"function_call","name":"shell","arguments":"{}","call_id":"c1"}"#,
        );
        let reply = line("10:02:00", "event_msg", r#"{"type":"agent_message","message":"Done."}"#);
        let path = Path::new("rollout-2026-11-12T10-00-00-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl");
        let t = parse_rfc3339("2026-11-12T10:02:00Z").unwrap();

        let p = parse(std::slice::from_ref(&prompt));
        assert_eq!(p.title.as_deref(), Some("Fix the login test"));
        assert_eq!(session(path, &p, t, t + 5).unwrap().status, Status::Working);

        let p = parse(&[prompt.clone(), call]);
        assert_eq!(session(path, &p, t, t + 60).unwrap().status, Status::NeedsInput);

        let p = parse(&[prompt, reply]);
        let s = session(path, &p, t, t + 60).unwrap();
        assert_eq!(s.status, Status::Done);
        assert_eq!(s.id, "codex:0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b");
        assert_eq!(s.project(), "shop");
        assert_eq!(session(path, &p, t, t + 2 * 3600).unwrap().status, Status::Idle);
    }

    #[test]
    fn reads_a_codex_folder() {
        let home = std::env::temp_dir().join(format!("trayward-codex-test-{}", std::process::id()));
        let day = home.join("sessions").join("2026").join("11").join("12");
        std::fs::create_dir_all(&day).unwrap();
        let now = parse_rfc3339("2026-11-12T10:10:00Z").unwrap();
        // A big file: its start and its end are read, not the middle.
        let filler = line("10:03:00", "event_msg", r#"{"type":"agent_reasoning","text":"thinking …"}"#);
        let mut big = vec![
            META.to_string(),
            line("10:01:00", "event_msg", r#"{"type":"user_message","message":"Refactor the API"}"#),
        ];
        big.extend(std::iter::repeat_n(filler, 2000));
        big.push(line(
            "10:05:00",
            "event_msg",
            r#"{"type":"token_count","rate_limits":{"primary":{"used_percent":22.0,"window_minutes":300,"resets_at":1794500000},"secondary":{"used_percent":55.0,"window_minutes":10080,"resets_at":1794900000},"plan_type":"pro"}}"#,
        ));
        big.push(line("10:06:00", "event_msg", r#"{"type":"agent_message","message":"Done."}"#));
        let path = day.join("rollout-2026-11-12T10-00-00-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl");
        std::fs::write(&path, big.join("\n") + "\n").unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > HEAD_BYTES + TAIL_BYTES / 2);
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs((now - 240) as u64);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(modified).unwrap();

        let mut reader = Reader::default();
        let snap = reader.read(&home, now);
        let limits = snap.limits.unwrap();
        assert_eq!(limits.five_hour.unwrap().used, 22.0);
        assert_eq!(limits.weekly.unwrap().used, 55.0);
        assert_eq!(snap.plan.as_deref(), Some("Pro"));
        assert_eq!(snap.sessions.len(), 1);
        let s = &snap.sessions[0];
        assert_eq!((s.status, s.title.as_str(), s.project()), (Status::Done, "Refactor the API", "shop"));
        // Unchanged files come from the cache: the same result.
        assert_eq!(reader.read(&home, now), snap);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn follows_a_session_in_an_old_days_folder() {
        let home = std::env::temp_dir().join(format!("trayward-codex-old-{}", std::process::id()));
        let day = home.join("sessions").join("2026").join("10").join("01");
        std::fs::create_dir_all(&day).unwrap();
        // The folder is from now; seen from a month later, it's an old day's. The
        // session in it is still written to then.
        let real_now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let now = real_now + 30 * 86_400;
        let path = day.join("rollout-2026-10-01T10-00-00-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl");
        let reply = line("10:06:00", "event_msg", r#"{"type":"agent_message","message":"Done."}"#);
        std::fs::write(&path, format!("{META}\n{reply}\n")).unwrap();
        let modified = UNIX_EPOCH + std::time::Duration::from_secs((now - 100) as u64);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(modified).unwrap();

        let sessions = home.join("sessions");
        assert!(recent_files(&sessions, now - LIMITS_DAYS * 86_400, |_| false).is_empty());
        // The first pass looks at every day, the next ones at this one again.
        let mut reader = Reader::default();
        assert_eq!(reader.read(&home, now).sessions.len(), 1);
        assert_eq!(reader.read(&home, now + 5).sessions.len(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn reads_the_line_across_the_starts_border() {
        let home = std::env::temp_dir().join(format!("trayward-codex-border-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let filler = line("10:03:00", "event_msg", r#"{"type":"agent_reasoning","text":"thinking …"}"#);
        let mut text = format!(
            "{META}\n{}\n",
            line("10:01:00", "event_msg", r#"{"type":"user_message","message":"Refactor the API"}"#)
        );
        while text.len() + filler.len() + 1 < HEAD_BYTES as usize - 10 {
            text += &filler;
            text.push('\n');
        }
        // The reply begins before the first 64 KB end, and ends after.
        text += &line("10:06:00", "event_msg", r#"{"type":"agent_message","message":"Done."}"#);
        text.push('\n');
        assert!(text.len() as u64 > HEAD_BYTES);
        let path = home.join("rollout.jsonl");
        std::fs::write(&path, &text).unwrap();
        let p = parse_file(&path, text.len() as u64);
        assert!(p.last_reply.is_some() && p.last_reply > p.last_prompt);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn skips_cut_off_lines() {
        let buf = b"{\"cut\": 1}\n{\"a\":1}\n{\"b\":";
        assert_eq!(complete_lines(buf, true).collect::<Vec<_>>(), ["{\"a\":1}"]);
        assert_eq!(complete_lines(buf, false).count(), 2);
        // A read that starts inside a character ("ü" is two bytes) still gives the lines after it.
        let cut = &"ü\"}\n{\"a\":\"ö\"}\n".as_bytes()[1..];
        assert_eq!(complete_lines(cut, true).collect::<Vec<_>>(), ["{\"a\":\"ö\"}"]);
    }

    #[test]
    fn reads_every_kind_of_step() {
        // No session_meta (cut off): the folder comes from the turn's context.
        let mut p = Parsed::default();
        parse_line(&mut p, &line("10:00:00", "turn_context", r#"{"cwd":"C:\\work\\api","model":"gpt-6-codex"}"#), true);
        assert_eq!((p.cwd.as_deref(), p.model.as_deref()), (Some("C:\\work\\api"), Some("gpt-6-codex")));
        // A later context without a model keeps the one before, and the folder stays.
        parse_line(&mut p, &line("10:00:01", "turn_context", r#"{"cwd":"C:\\elsewhere"}"#), true);
        assert_eq!((p.cwd.as_deref(), p.model.as_deref()), (Some("C:\\work\\api"), Some("gpt-6-codex")));
        // A prompt read from the end of the file isn't the title.
        parse_line(&mut p, &line("10:01:00", "event_msg", r#"{"type":"user_message","message":"later"}"#), false);
        assert_eq!((p.title.as_deref(), p.last_prompt), (None, parse_rfc3339("2026-11-12T10:01:00Z")));
        let step = |p: &mut Parsed, kind: &str, payload: &str| parse_line(p, &line("10:02:00", kind, payload), true);
        step(&mut p, "response_item", r#"{"type":"local_shell_call"}"#);
        assert!(p.open_call.is_some());
        step(&mut p, "response_item", r#"{"type":"custom_tool_call_output"}"#);
        assert!(p.open_call.is_none());
        step(&mut p, "response_item", r#"{"type":"custom_tool_call"}"#);
        step(&mut p, "event_msg", r#"{"type":"turn_aborted"}"#);
        assert!(p.open_call.is_none() && p.last_reply.is_some());
        let mut q = Parsed::default();
        step(&mut q, "response_item", r#"{"type":"message","role":"user"}"#);
        assert_eq!(q.last_reply, None);
        step(&mut q, "response_item", r#"{"type":"message","role":"assistant"}"#);
        assert!(q.last_reply.is_some());
        // Limits without a percentage aren't limits.
        step(&mut q, "event_msg", r#"{"type":"token_count","rate_limits":{"primary":{"window_minutes":300}}}"#);
        assert_eq!(q.limits, None);
        // A long first prompt is cut.
        let long = "x".repeat(130);
        assert_eq!(first_line(&long).chars().count(), 121);
        assert!(first_line(&long).ends_with('…'));
        assert_eq!(first_line("\n  \n  second  "), "second");
        assert_eq!(plan_name(""), "");
    }

    #[test]
    fn a_session_without_its_id_takes_the_files() {
        let p = Parsed { cwd: Some("C:\\work\\shop\\".into()), ..Parsed::default() };
        let path = Path::new("rollout-2026-11-12T10-00-00-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl");
        let s = session(path, &p, 100, 100 + 2 * IDLE_AFTER_SECS).unwrap();
        assert_eq!(
            (s.id.as_str(), s.status, s.project()),
            ("codex:0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b", Status::Idle, "shop")
        );
        // A short name: all of it.
        assert_eq!(session(Path::new("short.jsonl"), &p, 0, 0).unwrap().id, "codex:short");
        let none = Session { cwd: String::new(), ..s };
        assert_eq!(none.project(), "—");
    }

    #[test]
    fn takes_the_newest_limits_of_all_files() {
        let home = std::env::temp_dir().join(format!("trayward-codex-limits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let day = home.join("sessions").join("2026").join("11").join("12");
        std::fs::create_dir_all(&day).unwrap();
        let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let limits = |at: &str, used: f64, plan: &str| {
            line(
                at,
                "event_msg",
                &format!(
                    r#"{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":{used}}},"plan_type":"{plan}"}}}}"#
                ),
            )
        };
        let write = |name: &str, lines: &[String], changed: i64| {
            let path = day.join(name);
            std::fs::write(&path, lines.join("\n") + "\n").unwrap();
            let time = UNIX_EPOCH + std::time::Duration::from_secs(changed as u64);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(time).unwrap();
        };
        write("rollout-a.jsonl", &[META.to_string(), limits("10:05:00", 20.0, "plus")], now - 2 * 86_400);
        write("rollout-b.jsonl", &[META.to_string(), limits("10:01:00", 10.0, "pro")], now - 60);
        // Too old for the limits, and not a session file.
        write("rollout-c.jsonl", &[META.to_string(), limits("11:00:00", 90.0, "team")], now - 30 * 86_400);
        std::fs::write(day.join("notes.txt"), "x").unwrap();
        let snap = Reader::default().read(&home, now);
        // The file changed two days ago has the newer reading; only the recent one is listed.
        assert_eq!(
            (snap.limits.and_then(|l| l.five_hour).map(|l| l.used), snap.plan.as_deref()),
            (Some(20.0), Some("Plus"))
        );
        assert_eq!(snap.sessions.len(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn looks_only_while_on() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn(move |s| drop(tx.send(s)), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(), Snapshot::default());
    }
}
