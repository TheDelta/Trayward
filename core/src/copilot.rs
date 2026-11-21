//! GitHub Copilot as a third provider: the sessions of its agent (the Copilot
//! CLI and the agent in VS Code write the same files) and, if switched on,
//! the monthly limit of the plan.
//!
//! Sessions come from `~/.copilot/session-state/<id>/` (`COPILOT_HOME` moves
//! `~/.copilot`): no network, nothing written.
//! - `inuse.<pid>.lock`: the process running the session. Locks of processes
//!   that ended stay behind, and a pid can be reused: the process must have
//!   started before the lock was written.
//! - `workspace.yaml`: the folder and the session's name.
//! - `events.jsonl`: one event per line, `{"type", "data", "timestamp", …}`,
//!   appended as it happens. Used here: `session.start` / `session.resume`
//!   (id, folder), `session.shutdown`, `user.message`, `assistant.turn_start`
//!   / `assistant.turn_end`, `abort`, `tool.execution_start` /
//!   `tool.execution_complete` (the tool `ask_user` is a question to the user),
//!   `permission.requested` / `permission.completed`, `session.usage_record`
//!   (model, tokens in the context), `session.auto_mode_resolved` and
//!   `session.model_change` (model).
//!
//! The limit isn't in the files (Copilot keeps its quota readings in memory).
//! It comes from GitHub's `copilot_internal/user`, asked through `gh` with its
//! own login (never Trayward's token for github.com: that one is made for pull
//! requests and has no Copilot permission). Off unless switched on,
//! since it contacts GitHub. The endpoint is undocumented: parse defensively.

use crate::codex::Session;
use crate::sessions::Status;
use crate::timefmt::parse_rfc3339;
use crate::usage::Limit;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Sessions are listed when their events changed within a day.
const LIST_SECS: i64 = 24 * 3600;
/// A turn that ended this recently may be followed by the next one: still working.
const SETTLE_SECS: i64 = 2;
/// Done this long ago: idle.
const IDLE_AFTER_SECS: i64 = 3600;
/// How often the limit is asked for.
const QUOTA_EVERY: Duration = Duration::from_secs(5 * 60);
/// After a failure, it's asked again this much later, doubling with each
/// failure in a row up to [`QUOTA_RETRY_MAX`] (no Copilot on the account, gh
/// logged out, offline: no point asking every minute).
const QUOTA_RETRY: Duration = Duration::from_secs(60);
const QUOTA_RETRY_MAX: Duration = Duration::from_secs(30 * 60);

/// Which of the plan's allowances the limit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Premium requests (paid plans).
    Premium,
    /// Chat messages (the free plan).
    Chat,
}

/// Copilot's monthly limit.
#[derive(Clone, Debug, PartialEq)]
pub struct Quota {
    pub kind: Kind,
    pub limit: Limit,
    /// How many are left, and how many a month has.
    pub remaining: f64,
    pub entitlement: f64,
    /// The plan, e.g. "Free" or "Pro".
    pub plan: Option<String>,
    /// When GitHub was asked (unix seconds).
    pub at: i64,
}

/// What asking for the limit gave: the limit (none on a plan without one), or
/// why it failed.
pub type QuotaResult = Result<Option<Quota>, String>;

/// Copilot's folder, if Copilot's agent is used on this PC (the demo's own one in demo mode).
pub fn home() -> Option<PathBuf> {
    let dir = if crate::demo::active() {
        crate::demo::copilot_dir()?
    } else if let Some(h) = std::env::var_os("COPILOT_HOME").filter(|h| !h.is_empty()) {
        PathBuf::from(h)
    } else {
        PathBuf::from(std::env::var_os("USERPROFILE")?).join(".copilot")
    };
    dir.join("session-state").is_dir().then_some(dir)
}

/// Whether Copilot's sessions are read (the setting), for the statistics.
static ON: AtomicBool = AtomicBool::new(false);

pub fn set_on(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// The prefix of a Copilot session's id in Trayward.
pub const ID_PREFIX: &str = "copilot:";

/// Every Copilot session's events file, for the statistics: the session's
/// id (with its prefix) and the file. None while Copilot is switched off.
pub fn event_files() -> Vec<(String, PathBuf)> {
    if !ON.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let Some(home) = home() else { return Vec::new() };
    std::fs::read_dir(home.join("session-state"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let events = e.path().join("events.jsonl");
            events.is_file().then(|| (format!("{ID_PREFIX}{}", e.file_name().to_string_lossy()), events))
        })
        .collect()
}

/// The events file of the session `id` (with its prefix), if there is one.
pub fn events_of(id: &str) -> Option<PathBuf> {
    let folder = id.strip_prefix(ID_PREFIX).filter(|f| !f.is_empty() && !f.contains(['/', '\\', '.']))?;
    let events = home()?.join("session-state").join(folder).join("events.jsonl");
    (ON.load(Ordering::Relaxed) && events.is_file()).then_some(events)
}

/// The session's name in the `workspace.yaml` next to its events file.
pub fn name_of(events: &Path) -> Option<String> {
    yaml_value(&std::fs::read_to_string(events.with_file_name("workspace.yaml")).ok()?, "name")
}

/// What a session's events say, as far as Trayward needs it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Parsed {
    id: Option<String>,
    cwd: Option<String>,
    /// The first prompt.
    prompt: Option<String>,
    model: Option<String>,
    last_prompt: Option<i64>,
    /// A turn started and hasn't ended.
    turn_open: bool,
    /// When the last turn ended.
    ended: Option<i64>,
    /// Tool calls without a result: id, when it started, whether it asks the user.
    open_tools: Vec<(String, i64, bool)>,
    /// A permission was asked for (unix seconds) and not answered yet.
    permission: Option<i64>,
    /// The session was shut down (and not resumed).
    closed: bool,
    context_tokens: u64,
}

/// The event types that are read; the others are skipped without parsing
/// (messages and tool results can be large).
const WANTED: [&str; 13] = [
    "session.start",
    "session.resume",
    "session.shutdown",
    "session.usage_record",
    "session.auto_mode_resolved",
    "session.model_change",
    "user.message",
    "assistant.turn_start",
    "assistant.turn_end",
    "abort",
    "tool.execution_start",
    "tool.execution_complete",
    "permission.requested",
];
/// Read too; kept apart for the array's length.
const WANTED_MORE: [&str; 1] = ["permission.completed"];

/// The event's type when the line starts with it, as Copilot writes it.
pub(crate) fn kind_of(line: &str) -> Option<&str> {
    line.strip_prefix("{\"type\":\"")?.split('"').next()
}

fn parse_line(p: &mut Parsed, line: &str) {
    if kind_of(line).is_some_and(|k| !WANTED.contains(&k) && !WANTED_MORE.contains(&k)) {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let at = v["timestamp"].as_str().and_then(parse_rfc3339).unwrap_or(0);
    let data = &v["data"];
    let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    match v["type"].as_str().unwrap_or_default() {
        "session.start" | "session.resume" => {
            p.id = p.id.take().or_else(|| text(&data["sessionId"]));
            p.cwd = text(&data["context"]["cwd"]).or(p.cwd.take());
            p.closed = false;
            // A turn still open here was cut off (Copilot ended without saying so):
            // nothing of it runs or waits for the user any more.
            p.turn_open = false;
            p.open_tools.clear();
            p.permission = None;
        }
        "session.shutdown" => {
            p.closed = true;
            end_turn(p, at);
        }
        "session.auto_mode_resolved" => p.model = text(&data["chosenModel"]).or(p.model.take()),
        "session.model_change" => p.model = text(&data["newModel"]).or(p.model.take()),
        "session.usage_record" => {
            let usage = &data["usage"];
            p.model = text(&usage["modelDisplayName"]).or_else(|| text(&usage["model"])).or(p.model.take());
            // The request's input, cached parts included: what's in the context.
            if let Some(t) = usage["inputTokens"].as_u64() {
                p.context_tokens = t;
            }
        }
        "user.message" => {
            if p.prompt.is_none() {
                p.prompt = data["content"].as_str().map(crate::codex::first_line).filter(|l| !l.is_empty());
            }
            p.last_prompt = Some(at);
            p.closed = false;
        }
        "assistant.turn_start" => {
            p.turn_open = true;
            p.ended = None;
        }
        "assistant.turn_end" | "abort" => end_turn(p, at),
        "tool.execution_start" => {
            if let Some(id) = text(&data["toolCallId"]) {
                p.open_tools.retain(|(i, _, _)| *i != id);
                p.open_tools.push((id, at, data["toolName"] == "ask_user"));
            }
        }
        "tool.execution_complete" => {
            if let Some(id) = data["toolCallId"].as_str() {
                p.open_tools.retain(|(i, _, _)| i != id);
            }
        }
        "permission.requested" => p.permission = Some(at),
        "permission.completed" => p.permission = None,
        _ => {}
    }
}

fn end_turn(p: &mut Parsed, at: i64) {
    p.turn_open = false;
    p.ended = Some(at);
    p.open_tools.clear();
    p.permission = None;
}

/// A session's events, read up to `offset`.
#[derive(Default)]
struct Cached {
    tail: crate::jsonl::Tail,
    parsed: Parsed,
}

/// Reads what was added to `path`. A file that got shorter was replaced:
/// it's read from the start.
fn read_new(path: &Path, cached: &mut Cached) {
    loop {
        let Cached { tail, parsed } = cached;
        if tail.read(path, None, |line| parse_line(parsed, line)) != crate::jsonl::Outcome::Replaced {
            return;
        }
        *cached = Cached::default();
    }
}

/// The value of `key: value` in a simple YAML file, without its quotes.
fn yaml_value(text: &str, key: &str) -> Option<String> {
    let value = text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))?.trim();
    let unquoted = if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        inner.replace("''", "'")
    } else {
        value.to_string()
    };
    (!unquoted.is_empty()).then_some(unquoted)
}

fn mtime(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// The processes that hold a lock on the session in `dir`: pid and when the
/// lock was written.
fn locks(dir: &Path) -> Vec<(u32, i64)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let pid = name.to_str()?.strip_prefix("inuse.")?.strip_suffix(".lock")?.parse().ok()?;
            Some((pid, mtime(&e.metadata().ok()?)))
        })
        .collect()
}

/// Whether the process `pid` runs and is the one that wrote a lock at `locked_at`.
fn holds_lock(pid: u32, locked_at: i64) -> bool {
    crate::os::process_alive(pid, None) && crate::os::started_by(pid, locked_at)
}

/// Reads Copilot's sessions, reading only what was added to their events.
#[derive(Default)]
pub struct Reader {
    cache: HashMap<PathBuf, Cached>,
}

impl Reader {
    /// The running sessions; `running` says whether a lock's process (pid,
    /// when the lock was written) still holds it.
    pub fn read(&mut self, home: &Path, now: i64, running: impl Fn(u32, i64) -> bool) -> Vec<Session> {
        let mut sessions = Vec::new();
        let mut seen = Vec::new();
        for entry in std::fs::read_dir(home.join("session-state")).into_iter().flatten().flatten() {
            let dir = entry.path();
            let events = dir.join("events.jsonl");
            let Ok(meta) = std::fs::metadata(&events) else { continue };
            let changed = mtime(&meta);
            if now - changed > LIST_SECS || !locks(&dir).iter().any(|(pid, at)| running(*pid, *at)) {
                continue;
            }
            let cached = self.cache.entry(events.clone()).or_default();
            read_new(&events, cached);
            let workspace = std::fs::read_to_string(dir.join("workspace.yaml")).unwrap_or_default();
            let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some(s) = session(&folder, &cached.parsed, &workspace, changed, now) {
                sessions.push(s);
            }
            seen.push(events);
        }
        self.cache.retain(|path, _| seen.contains(path));
        sessions.sort_by_key(|s| std::cmp::Reverse(s.since));
        sessions
    }
}

/// The session in the folder `folder`, whose events last changed at `changed`.
fn session(folder: &str, p: &Parsed, workspace: &str, changed: i64, now: i64) -> Option<Session> {
    if p.closed {
        return None;
    }
    // The folder's name: the statistics and the resume command find the session by it.
    let id = if folder.is_empty() { p.id.clone()? } else { folder.to_string() };
    let quiet = now - changed;
    let asked = p.open_tools.iter().filter(|(_, _, ask)| *ask).map(|(_, at, _)| *at).chain(p.permission).max();
    let (status, since) = match (p.last_prompt, p.ended) {
        (None, _) => (Status::Idle, changed),
        // Between two turns of one answer, the file is quiet only for a moment.
        (Some(prompt), ended) if p.turn_open || ended.is_none() || quiet < SETTLE_SECS => match asked {
            Some(at) if p.turn_open => (Status::NeedsInput, at),
            _ => (Status::Working, prompt),
        },
        (Some(_), Some(ended)) if quiet < IDLE_AFTER_SECS => (Status::Done, ended),
        _ => (Status::Idle, changed),
    };
    Some(Session {
        id: format!("copilot:{id}"),
        cwd: yaml_value(workspace, "cwd").or_else(|| p.cwd.clone()).unwrap_or_default(),
        title: yaml_value(workspace, "name").or_else(|| p.prompt.clone()).unwrap_or_default(),
        status,
        since,
        model: p.model.clone(),
        context_tokens: p.context_tokens,
    })
}

/// Reads Copilot's sessions every few seconds on its own thread and posts
/// them to `send` when something changed. Looks only while `on`.
pub fn spawn(send: impl Fn(Vec<Session>) + Send + 'static, on: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        crate::os::background_thread();
        let mut reader = Reader::default();
        let mut last: Option<Vec<Session>> = None;
        loop {
            let sessions = if on.load(Ordering::Relaxed) {
                home().map(|h| reader.read(&h, crate::os::now_ms() / 1000, holds_lock))
            } else {
                None
            };
            let sessions = sessions.unwrap_or_default();
            if last.as_ref() != Some(&sessions) {
                last = Some(sessions.clone());
                send(sessions);
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}

// ---------- the limit ----------

/// The plan's name from GitHub's answer.
fn plan_name(v: &Value) -> Option<String> {
    let sku = v["access_type_sku"].as_str().unwrap_or_default();
    let plan = v["copilot_plan"].as_str().unwrap_or_default();
    let name = match plan {
        _ if sku.contains("free") => "Free".to_string(),
        _ if sku.contains("plus") => "Pro+".to_string(),
        "individual" => "Pro".to_string(),
        "" => return None,
        other => crate::codex::plan_name(&other.replace('_', " ")),
    };
    Some(name)
}

/// The monthly limit in GitHub's answer: premium requests on a plan that has
/// them, else the free plan's chat messages; none when neither is limited.
fn quota(v: &Value, now: i64) -> Option<Quota> {
    let resets_at = v["quota_reset_date_utc"]
        .as_str()
        .and_then(parse_rfc3339)
        .or_else(|| parse_rfc3339(&format!("{}T00:00:00Z", v["quota_reset_date"].as_str()?)))
        .filter(|t| *t > 0);
    [("premium_interactions", Kind::Premium), ("chat", Kind::Chat)].into_iter().find_map(|(key, kind)| {
        let s = &v["quota_snapshots"][key];
        let entitlement = s["entitlement"].as_f64().filter(|e| *e > 0.0)?;
        if s["unlimited"].as_bool() == Some(true) {
            return None;
        }
        let remaining = s["quota_remaining"].as_f64().or_else(|| s["remaining"].as_f64());
        let left = s["percent_remaining"].as_f64().or_else(|| remaining.map(|r| r / entitlement * 100.0))?;
        Some(Quota {
            kind,
            limit: Limit { used: (100.0 - left).clamp(0.0, 100.0), resets_at },
            remaining: remaining.unwrap_or(entitlement * left / 100.0).max(0.0),
            entitlement,
            plan: plan_name(v),
            at: now,
        })
    })
}

/// Asks GitHub through `gh`.
fn fetch(now: i64) -> QuotaResult {
    let github = crate::forge::Forge::GitHub;
    let answer = crate::forge::run_with_own_login(
        github,
        "github.com",
        &["api", "--hostname", "github.com", "copilot_internal/user"],
    )?;
    let v: Value = serde_json::from_str(&answer).map_err(|e| format!("gh: {e}"))?;
    // An error comes as JSON too: {"message": "…", "status": "401"}.
    if v["quota_snapshots"].is_null() {
        return Err(v["message"].as_str().unwrap_or("gh: no quota in the answer").to_string());
    }
    Ok(quota(&v, now))
}

/// How long to wait before asking again, after `failures` failures in a row.
fn retry_after(failures: u32) -> Duration {
    match failures {
        0 => QUOTA_EVERY,
        n => QUOTA_RETRY.saturating_mul(1 << (n - 1).min(10)).min(QUOTA_RETRY_MAX),
    }
}

/// Asks for the limit every few minutes (and right away when `poke` is set)
/// while `on`, and posts a [`QuotaResult`] to `send` when it changed.
pub fn spawn_quota(send: impl Fn(QuotaResult) + Send + 'static, on: Arc<AtomicBool>, poke: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        crate::os::background_thread();
        let mut was_on = false;
        let mut next: Option<Instant> = None;
        let mut failures = 0u32;
        loop {
            if !on.load(Ordering::Relaxed) {
                next = None;
                failures = 0;
                if std::mem::take(&mut was_on) {
                    send(Ok(None));
                }
            } else if poke.swap(false, Ordering::Relaxed) || next.is_none_or(|t| Instant::now() >= t) {
                was_on = true;
                let now = crate::os::now_ms() / 1000;
                let result = if crate::demo::active() { Ok(Some(crate::demo::copilot_quota(now))) } else { fetch(now) };
                failures = if result.is_ok() { 0 } else { failures + 1 };
                next = Some(Instant::now() + retry_after(failures));
                // Every answer, also an unchanged one: the flyout shows when it was asked.
                send(result);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(at: &str, kind: &str, data: &str) -> String {
        format!(r#"{{"type":"{kind}","data":{data},"id":"e1","timestamp":"2026-11-12T{at}.000Z","parentId":null}}"#)
    }

    fn start() -> String {
        line(
            "10:00:00",
            "session.start",
            r#"{"sessionId":"e94d9f6e-0000-4000-8000-000000000001","version":1,"producer":"copilot-agent","copilotVersion":"1.0.100-1","selectedModel":"auto","context":{"cwd":"C:\\work\\shop","branch":"main"}}"#,
        )
    }

    fn parse(lines: &[String]) -> Parsed {
        let mut p = Parsed::default();
        for l in lines {
            parse_line(&mut p, l);
        }
        p
    }

    const WORKSPACE: &str = "cwd: C:\\work\\shop\nid: e94d9f6e-0000-4000-8000-000000000001\nclient_name: vscode-agent\nname: \"Fix the \\\"cart\\\" total\"\nuser_named: false\n";

    fn t(clock: &str) -> i64 {
        parse_rfc3339(&format!("2026-11-12T{clock}Z")).unwrap()
    }

    #[test]
    fn tells_the_status() {
        let prompt = line("10:01:00", "user.message", r#"{"content":"Fix the cart total\nmore","turnId":"0"}"#);
        let turn = line("10:01:01", "assistant.turn_start", r#"{"turnId":"0"}"#);
        let usage = line(
            "10:01:05",
            "session.usage_record",
            r#"{"usage":{"model":"gpt-6-luna","modelDisplayName":"GPT-6 Luna","inputTokens":31125,"outputTokens":377,"cacheReadTokens":2304,"cost":1.0}}"#,
        );
        let tool = |name: &str| {
            line("10:01:06", "tool.execution_start", &format!(r#"{{"toolCallId":"call_1","toolName":"{name}"}}"#))
        };
        let result = line("10:01:30", "tool.execution_complete", r#"{"toolCallId":"call_1","success":true}"#);
        let end = line("10:02:00", "assistant.turn_end", r#"{"turnId":"0"}"#);
        let at = |p: &Parsed, changed: &str, now: &str| session("folder", p, WORKSPACE, t(changed), t(now)).unwrap();

        // No prompt yet: idle.
        assert_eq!(at(&parse(&[start()]), "10:00:00", "10:00:30").status, Status::Idle);

        // A turn runs, also while a tool takes long.
        let p = parse(&[start(), prompt.clone(), turn.clone(), usage.clone(), tool("powershell")]);
        let s = at(&p, "10:01:06", "10:05:00");
        assert_eq!((s.status, s.since), (Status::Working, t("10:01:00")));
        assert_eq!((s.model.as_deref(), s.context_tokens), (Some("GPT-6 Luna"), 31_125));
        // The id is the folder's name, which the statistics look the session up by.
        assert_eq!((s.id.as_str(), s.project()), ("copilot:folder", "shop"));
        // The name from workspace.yaml is the title.
        assert_eq!(s.title, "Fix the \"cart\" total");

        // A question to the user, until it's answered.
        let mut lines = vec![start(), prompt.clone(), turn.clone(), tool("ask_user")];
        let s = at(&parse(&lines), "10:01:06", "10:01:07");
        assert_eq!((s.status, s.since), (Status::NeedsInput, t("10:01:06")));
        lines.push(result.clone());
        assert_eq!(at(&parse(&lines), "10:01:30", "10:01:31").status, Status::Working);

        // A permission prompt, until it's answered.
        let ask = line("10:01:07", "permission.requested", r#"{"requestId":"r1"}"#);
        let mut lines = vec![start(), prompt.clone(), turn.clone(), tool("powershell"), ask];
        assert_eq!(at(&parse(&lines), "10:01:07", "10:03:00").status, Status::NeedsInput);
        lines.push(line("10:01:20", "permission.completed", r#"{"requestId":"r1"}"#));
        assert_eq!(at(&parse(&lines), "10:01:20", "10:03:00").status, Status::Working);

        // The turn ended: working for a moment (the next turn may follow), then done, then idle.
        let p = parse(&[start(), prompt.clone(), turn.clone(), tool("powershell"), result, end]);
        assert_eq!(at(&p, "10:02:00", "10:02:01").status, Status::Working);
        let s = at(&p, "10:02:00", "10:02:30");
        assert_eq!((s.status, s.since), (Status::Done, t("10:02:00")));
        assert_eq!(at(&p, "10:02:00", "11:30:00").status, Status::Idle);

        // Stopped by the user while a question was open: done, nothing open.
        let stop = line("10:01:40", "abort", r#"{"reason":"user_initiated"}"#);
        let p = parse(&[start(), prompt.clone(), turn.clone(), tool("ask_user"), stop]);
        assert_eq!(at(&p, "10:01:40", "10:02:00").status, Status::Done);
        assert!(p.open_tools.is_empty());

        // Shut down: not listed; resumed: listed again.
        let mut lines = vec![start(), prompt, turn, line("10:03:00", "session.shutdown", "{}")];
        assert_eq!(session("folder", &parse(&lines), WORKSPACE, t("10:03:00"), t("10:03:05")), None);
        lines.push(line("10:10:00", "session.resume", r#"{"context":{"cwd":"C:\\work\\shop"}}"#));
        assert!(session("folder", &parse(&lines), WORKSPACE, t("10:10:00"), t("10:10:05")).is_some());
    }

    #[test]
    fn skips_what_it_does_not_know() {
        let mut p = parse(&[start()]);
        let before = p.clone();
        for l in [
            // Not read at all: its type isn't wanted (and it isn't even JSON).
            r#"{"type":"assistant.message","data":{"content": cut off"#,
            r#"{"type":"hook.start","data":{"hookType":"preToolUse"}}"#,
            // Wanted types with unexpected content.
            r#"{"type":"tool.execution_start","data":{"toolCallId":7}}"#,
            r#"{"type":"session.usage_record","data":{"usage":"none"}}"#,
            r#"{"type":"user.message"}"#,
            "not json",
            "",
        ] {
            parse_line(&mut p, l);
        }
        // Only the prompt without content left a trace: when it came (unknown: 0).
        assert_eq!(p, Parsed { last_prompt: Some(0), ..before });
        // Another key order is read too.
        let mut p = Parsed::default();
        parse_line(
            &mut p,
            r#"{"timestamp":"2026-11-12T10:00:00Z","data":{"turnId":"0"},"type":"assistant.turn_start"}"#,
        );
        assert!(p.turn_open);
    }

    #[test]
    fn reads_simple_yaml() {
        assert_eq!(yaml_value(WORKSPACE, "cwd").as_deref(), Some("C:\\work\\shop"));
        assert_eq!(yaml_value(WORKSPACE, "name").as_deref(), Some("Fix the \"cart\" total"));
        assert_eq!(yaml_value("name: 'It''s done'\n", "name").as_deref(), Some("It's done"));
        assert_eq!(yaml_value("name: plain: text\n", "name").as_deref(), Some("plain: text"));
        // Another key that starts the same, an empty value, a missing key.
        assert_eq!(yaml_value("name_long: x\nname: \n", "name"), None);
        assert_eq!(yaml_value(WORKSPACE, "branch"), None);
    }

    #[test]
    fn reads_a_copilot_folder() {
        let home = std::env::temp_dir().join(format!("trayward-copilot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let now = t("10:10:00");
        let touch = |path: &Path, at: i64| {
            let time = UNIX_EPOCH + Duration::from_secs(at as u64);
            std::fs::File::options().write(true).open(path).unwrap().set_modified(time).unwrap();
        };
        let make = |name: &str, pid: u32, lines: &[String], changed: i64| {
            let dir = home.join("session-state").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("events.jsonl"), lines.join("\n") + "\n").unwrap();
            touch(&dir.join("events.jsonl"), changed);
            std::fs::write(dir.join(format!("inuse.{pid}.lock")), pid.to_string()).unwrap();
            std::fs::write(dir.join("workspace.yaml"), WORKSPACE).unwrap();
            dir
        };
        let prompt = line("10:01:00", "user.message", r#"{"content":"Fix the cart total"}"#);
        let turn = line("10:01:01", "assistant.turn_start", r#"{"turnId":"0"}"#);
        let running = make("running", 11, &[start(), prompt.clone(), turn.clone()], now - 60);
        make("ended", 22, &[start(), prompt.clone(), turn.clone()], now - 60);
        make("old", 11, &[start(), prompt, turn], now - 2 * LIST_SECS);
        // A folder without events, and a file: skipped.
        std::fs::create_dir_all(home.join("session-state").join("empty")).unwrap();
        std::fs::write(home.join("session-state").join("file.txt"), "x").unwrap();

        let alive = |pid: u32, _: i64| pid == 11;
        let mut reader = Reader::default();
        let sessions = reader.read(&home, now, alive);
        assert_eq!(sessions.len(), 1);
        assert_eq!((sessions[0].status, sessions[0].project()), (Status::Working, "shop"));
        assert_eq!(reader.cache.len(), 1);

        // More is appended, the last line still being written: only whole lines are read.
        let events = running.join("events.jsonl");
        let end = line("10:09:00", "assistant.turn_end", r#"{"turnId":"0"}"#);
        let mut file = std::fs::File::options().append(true).open(&events).unwrap();
        std::io::Write::write_all(&mut file, format!("{end}\n{{\"type\":\"user.mess").as_bytes()).unwrap();
        drop(file);
        touch(&events, now - 30);
        let sessions = reader.read(&home, now, alive);
        assert_eq!(sessions[0].status, Status::Done);
        let offset = reader.cache[&events].tail.lines_end();
        assert_eq!(offset, std::fs::metadata(&events).unwrap().len() - "{\"type\":\"user.mess".len() as u64);

        // Replaced by a shorter file: read from the start.
        std::fs::write(&events, start() + "\n").unwrap();
        touch(&events, now - 10);
        assert_eq!(reader.read(&home, now, alive)[0].status, Status::Idle);

        // Its process ended: gone, and forgotten.
        assert!(reader.read(&home, now, |_, _| false).is_empty());
        assert!(reader.cache.is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn asks_less_often_while_it_fails() {
        assert_eq!(retry_after(0), QUOTA_EVERY);
        assert_eq!(retry_after(1), Duration::from_secs(60));
        assert_eq!(retry_after(2), Duration::from_secs(120));
        assert_eq!(retry_after(5), Duration::from_secs(960));
        assert_eq!(retry_after(6), QUOTA_RETRY_MAX);
        assert_eq!(retry_after(99), QUOTA_RETRY_MAX);
    }

    #[test]
    fn reads_the_monthly_limit() {
        let now = t("10:00:00");
        // The free plan: chat messages; no premium requests.
        let free: Value = serde_json::from_str(
            r#"{"access_type_sku":"free_limited_copilot","copilot_plan":"individual","quota_snapshots":{
            "chat":{"percent_remaining":99.6,"quota_remaining":199.3,"unlimited":false,"remaining":199,"entitlement":200},
            "completions":{"percent_remaining":100.0,"quota_remaining":2000.0,"unlimited":false,"entitlement":2000},
            "premium_interactions":{"percent_remaining":0.0,"quota_remaining":0.0,"unlimited":false,"has_quota":false,"entitlement":0}},
            "quota_reset_date":"2026-12-01","quota_reset_date_utc":"2026-12-01T00:00:00.000Z"}"#,
        )
        .unwrap();
        let q = quota(&free, now).unwrap();
        assert_eq!((q.kind, q.plan.as_deref(), q.entitlement, q.remaining), (Kind::Chat, Some("Free"), 200.0, 199.3));
        assert!((q.limit.used - 0.4).abs() < 1e-9);
        assert_eq!(q.limit.resets_at, parse_rfc3339("2026-12-01T00:00:00Z"));

        // A paid plan: premium requests; chat is unlimited. Only the date, no percentage.
        let paid: Value = serde_json::from_str(
            r#"{"access_type_sku":"monthly_subscriber","copilot_plan":"individual","quota_snapshots":{
            "chat":{"unlimited":true,"entitlement":0},
            "premium_interactions":{"remaining":225,"unlimited":false,"entitlement":300}},
            "quota_reset_date":"2026-12-01"}"#,
        )
        .unwrap();
        let q = quota(&paid, now).unwrap();
        assert_eq!((q.kind, q.plan.as_deref(), q.limit.used), (Kind::Premium, Some("Pro"), 25.0));
        assert_eq!(q.limit.resets_at, parse_rfc3339("2026-12-01T00:00:00Z"));

        // Nothing limited, or nothing understood: no limit.
        let unlimited: Value = serde_json::from_str(
            r#"{"copilot_plan":"business","quota_snapshots":{"chat":{"unlimited":true,"entitlement":0},"premium_interactions":{"unlimited":true,"entitlement":1000,"percent_remaining":100}}}"#,
        )
        .unwrap();
        assert_eq!(quota(&unlimited, now), None);
        assert_eq!(quota(&serde_json::json!({"quota_snapshots": "soon"}), now), None);
        assert_eq!(plan_name(&unlimited).as_deref(), Some("Business"));
        assert_eq!(
            plan_name(&serde_json::json!({"access_type_sku": "plus_monthly_subscriber_quota"})).as_deref(),
            Some("Pro+")
        );
    }
}
