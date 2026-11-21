//! Statistics from session transcripts: per session, for any period over all
//! sessions, and how much disk space Claude Code's files take.
//!
//! Transcripts are large (tool results are stored in full), so lines are
//! filtered by cheap substring checks first and only the small ones that
//! matter are parsed. Every reply, prompt and tool call is kept with its time,
//! so any period can be summed up exactly. Results are cached per file and
//! only appended lines are read again, so reopening the statistics is quick.
//! With kept statistics (`keep`), a session's numbers are also on disk, with
//! how far its transcript was read: a later run goes on from there instead of
//! reading everything again (`session_stats`). Folder sizes are cached for a
//! few minutes.
//!
//! The transcript format is internal to Claude Code; unexpected lines are
//! skipped.

use crate::registry::Source;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const HOUR: i64 = 3600;
pub const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;

/// What transcripts are counted by: raised with every change that makes a
/// transcript count differently (a fix, or something new that's read from it).
/// Kept statistics say which number made them; ones from another number are
/// made again from their transcript, as long as it exists.
pub const PARSER: u32 = 1;

/// A pause in activity this long ends a turn.
const TURN_PAUSE: i64 = 30 * 60;
/// In the statistics, what's kept of a session is written again once this much
/// of its transcripts is new (`keep` brings every session up to date itself,
/// every few hours): how much a later run reads at most to catch up.
const KEEP_AFTER: u64 = 4 << 20;
/// Longest text kept for a tool call's detail and error.
const DETAIL_CHARS: usize = 200;
const ERROR_CHARS: usize = 300;
/// Tool results larger than this aren't parsed for their error text.
const MAX_RESULT_LINE: usize = 2 << 20;

// Claude Code's tools, then Copilot's.
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit", "edit", "create", "str_replace_editor"];
const SHELL_TOOLS: &[&str] = &["Bash", "PowerShell", "bash", "powershell", "shell"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tokens {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// The part of `cache_write` kept for an hour (billed higher than 5 minutes).
    pub cache_write_1h: u64,
    pub output: u64,
    pub thinking: u64,
}

/// API prices of a model in dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Price {
    input: f64,
    output: f64,
    cache_read: f64,
}

/// First-party API prices by model id prefix (most specific first). Cache
/// writes cost 1.25x input for 5 minutes and 2x for an hour.
const PRICES: &[(&str, Price)] = &[
    ("fable-5-2", Price { input: 8.0, output: 40.0, cache_read: 0.20 }),
    ("mythos-5-2", Price { input: 8.0, output: 40.0, cache_read: 0.20 }),
    ("fable-5-1", Price { input: 10.0, output: 50.0, cache_read: 0.25 }),
    ("mythos-5-1", Price { input: 10.0, output: 50.0, cache_read: 0.25 }),
    ("fable-5", Price { input: 10.0, output: 50.0, cache_read: 1.0 }),
    ("mythos-5", Price { input: 10.0, output: 50.0, cache_read: 1.0 }),
    ("opus-5-5", Price { input: 4.0, output: 20.0, cache_read: 0.20 }),
    ("opus-5", Price { input: 5.0, output: 25.0, cache_read: 0.50 }),
    ("opus-4-8", Price { input: 5.0, output: 25.0, cache_read: 0.50 }),
    ("opus-4-7", Price { input: 5.0, output: 25.0, cache_read: 0.50 }),
    ("opus-4-6", Price { input: 5.0, output: 25.0, cache_read: 0.50 }),
    ("opus-4-5", Price { input: 5.0, output: 25.0, cache_read: 0.50 }),
    ("opus-4", Price { input: 15.0, output: 75.0, cache_read: 1.50 }),
    ("opus-3", Price { input: 15.0, output: 75.0, cache_read: 1.50 }),
    ("sonnet-5-5", Price { input: 2.0, output: 10.0, cache_read: 0.10 }),
    ("sonnet-5", Price { input: 2.0, output: 10.0, cache_read: 0.20 }),
    ("sonnet-4", Price { input: 3.0, output: 15.0, cache_read: 0.30 }),
    ("sonnet-3", Price { input: 3.0, output: 15.0, cache_read: 0.30 }),
    ("haiku-5-5", Price { input: 0.10, output: 0.50, cache_read: 0.01 }),
    ("haiku-4-5", Price { input: 1.0, output: 5.0, cache_read: 0.10 }),
    ("haiku-3-5", Price { input: 0.80, output: 4.0, cache_read: 0.08 }),
    ("haiku-3", Price { input: 0.25, output: 1.25, cache_read: 0.03 }),
];

/// A model id with its family first, the order the prices go by: the ids of
/// Claude 3 have the version first (`3-7-sonnet-20250219` is `sonnet-3-7-…`
/// here); newer ones stay as they are.
fn family_first(id: &str) -> std::borrow::Cow<'_, str> {
    if !id.starts_with(|c: char| c.is_ascii_digit()) {
        return id.into();
    }
    let parts: Vec<&str> = id.split('-').collect();
    let versions = parts.iter().take_while(|p| p.len() <= 2 && p.bytes().all(|b| b.is_ascii_digit())).count();
    if versions == 0 || versions >= parts.len() {
        return id.into();
    }
    let (first, rest) = parts.split_at(versions);
    let mut out: Vec<&str> = rest.iter().take(1).chain(first).copied().collect();
    out.extend(rest.iter().skip(1));
    out.join("-").into()
}

/// What a reply of `model` would cost at API prices, in dollars.
pub fn cost(model: &str, t: &Tokens) -> f64 {
    let id = family_first(model.strip_prefix("claude-").unwrap_or(model));
    let Some((_, p)) = PRICES.iter().find(|(prefix, _)| id.starts_with(prefix)) else { return 0.0 };
    // Haiku 5.5 costs five times as much for prompts over 100k tokens.
    let scale = if id.starts_with("haiku-5-5") && t.context() > 100_000 { 5.0 } else { 1.0 };
    let short_writes = t.cache_write.saturating_sub(t.cache_write_1h);
    (t.input as f64 * p.input
        + short_writes as f64 * p.input * 1.25
        + t.cache_write_1h as f64 * p.input * 2.0
        + t.cache_read as f64 * p.cache_read
        + t.output as f64 * p.output)
        * scale
        / 1e6
}

/// Dollars in a short form: `$0.42`, `$12.30`, `$1.2k`.
pub fn dollars(d: f64) -> String {
    match d {
        d if d >= 10_000.0 => format!("${:.0}k", d / 1000.0),
        d if d >= 1000.0 => format!("${:.1}k", d / 1000.0),
        d if d >= 100.0 => format!("${d:.0}"),
        d => format!("${d:.2}"),
    }
}

impl Tokens {
    pub fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.cache_write_1h += o.cache_write_1h;
        self.output += o.output;
        self.thinking += o.thinking;
    }

    /// The whole prompt sent with a reply.
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    /// Share of input tokens served from the prompt cache.
    pub fn cache_hit(&self) -> Option<f64> {
        let all = self.context();
        (all > 0).then(|| self.cache_read as f64 / all as f64)
    }
}

/// One reply of Claude.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reply {
    pub at: i64,
    /// Index into [`Stats::models`].
    pub model: Option<u16>,
    pub tokens: Tokens,
    pub web: u32,
}

/// One tool call.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Call {
    pub at: i64,
    pub tool: String,
    /// What it worked on: the command, path, pattern, URL…, shortened.
    pub detail: String,
    /// The file it read or changed.
    pub path: Option<String>,
    /// Lines written and replaced by edits.
    pub added: u32,
    pub removed: u32,
    /// The error it returned, shortened.
    pub error: Option<String>,
}

impl Call {
    pub fn edits(&self) -> bool {
        EDIT_TOOLS.contains(&self.tool.as_str())
    }

    pub fn shell(&self) -> bool {
        SHELL_TOOLS.contains(&self.tool.as_str())
    }

    pub fn agent(&self) -> bool {
        matches!(self.tool.as_str(), "Agent" | "Task")
    }

    pub fn commit(&self) -> bool {
        self.shell() && self.detail.contains("git commit")
    }

    /// A web search or fetch run by Claude Code (rather than on the server).
    pub fn web(&self) -> bool {
        matches!(self.tool.as_str(), "WebSearch" | "WebFetch")
    }

    /// What kind of failure this is, so that repeated ones group together:
    /// the program (or tool) and the error's first telling line, with
    /// numbers and paths left out. `None` if the call didn't fail.
    pub fn signature(&self) -> Option<String> {
        let e = self.error.as_deref().filter(|_| !self.declined())?;
        let lines: Vec<&str> = e.split(" ↵ ").map(str::trim).filter(|l| !l.is_empty()).collect();
        let line = match lines.iter().position(|l| l.starts_with("Traceback")) {
            // Python names the error at the end.
            Some(_) => lines.last().copied().unwrap_or(""),
            None => lines.iter().find(|l| !l.starts_with("Exit code")).or(lines.first()).copied().unwrap_or(""),
        };
        let words: Vec<String> = line
            .split_whitespace()
            .map(|w| {
                if w.len() > 3 && (w.contains('\\') || w.contains('/')) {
                    "…".to_string()
                } else {
                    let mut out = String::new();
                    for ch in w.chars() {
                        let ch = if ch.is_ascii_digit() { '#' } else { ch };
                        if !(ch == '#' && out.ends_with('#')) {
                            out.push(ch);
                        }
                    }
                    out
                }
            })
            .collect();
        let text = shorten(&words.join(" "), 90);
        // The last program of a command line is the likely culprit; earlier
        // ones tend to be set-up (changing folders, loading tools).
        let who = if self.shell() { programs(&self.detail).pop() } else { None };
        Some(format!("{}: {text}", who.unwrap_or_else(|| self.tool.clone())))
    }

    /// Declined by the user or blocked by a permission check rather than failed.
    pub fn declined(&self) -> bool {
        self.error.as_deref().is_some_and(|e| {
            e.starts_with("The user doesn't want to proceed")
                || e.starts_with("[Request interrupted")
                || (e.starts_with("Permission for this") && e.contains("denied"))
        })
    }
}

/// A prompt and when it was sent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prompt {
    pub at: i64,
    /// Its start, shortened.
    pub text: String,
}

/// One turn: a prompt and how long Claude worked on it (until its last reply
/// or tool call before the next prompt). Includes time spent waiting for you
/// to answer a question or permission prompt in between.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    pub at: i64,
    pub secs: i64,
    pub prompt: String,
}

/// What one transcript tells. Saved as is for kept statistics (`keep`), which
/// can't be made again once the transcript is gone: this struct and the ones in
/// it take a missing field as its default, so a later version's new field
/// doesn't make the files of today unreadable. What reading the transcript on
/// needs (the last four fields) is saved with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    pub prompts: Vec<Prompt>,
    pub replies: Vec<Reply>,
    pub models: Vec<String>,
    pub calls: Vec<Call>,
    pub compactions: Vec<i64>,
    /// Errors whose tool call wasn't found.
    pub orphan_errors: u32,
    /// What the session loaded: skills, MCP servers, instructions, memory.
    pub setup: crate::setup::Setup,
    /// Replies, tool calls and prompts already counted (see
    /// `transcripts::once_key`): lines can be written again later in the file.
    #[serde(skip_serializing_if = "HashSet::is_empty")]
    counted: HashSet<u64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    custom_title: bool,
    /// Tool call ids to their index in `calls`, to attach errors.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    open: HashMap<String, usize>,
    /// In a fork's transcript, until its first prompt: the reply before it is
    /// the one of the session that started the fork, counted there.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    fork_head: bool,
}

/// Sums over a time window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub prompts: u32,
    pub replies: u32,
    pub tool_calls: u32,
    pub errors: u32,
    pub declined: u32,
    pub agents: u32,
    pub web: u32,
    pub compactions: u32,
    pub tokens: Tokens,
    /// Output tokens written by subagents.
    pub agent_output: u64,
    pub added: u64,
    pub removed: u64,
    pub commits: u32,
    pub files_changed: u32,
    pub files_read: u32,
    /// What it would have cost at API prices (dollars), Claude's own replies.
    pub cost: f64,
    /// The same for subagents.
    pub agent_cost: f64,
    /// How long each turn took (seconds).
    pub turns: Vec<i64>,
}

impl Summary {
    pub fn add(&mut self, o: &Summary) {
        self.prompts += o.prompts;
        self.replies += o.replies;
        self.tool_calls += o.tool_calls;
        self.errors += o.errors;
        self.declined += o.declined;
        self.agents += o.agents;
        self.web += o.web;
        self.compactions += o.compactions;
        self.tokens.add(&o.tokens);
        self.agent_output += o.agent_output;
        self.added += o.added;
        self.removed += o.removed;
        self.commits += o.commits;
        self.files_changed += o.files_changed;
        self.files_read += o.files_read;
        self.cost += o.cost;
        self.agent_cost += o.agent_cost;
        self.turns.extend_from_slice(&o.turns);
    }

    /// Everything at API prices, including subagents.
    pub fn total_cost(&self) -> f64 {
        self.cost + self.agent_cost
    }

    /// Time Claude spent on turns (seconds).
    pub fn working_secs(&self) -> i64 {
        self.turns.iter().sum()
    }

    /// The middle turn length (seconds).
    pub fn median_turn(&self) -> Option<i64> {
        let mut t = self.turns.clone();
        t.sort_unstable();
        t.get(t.len() / 2).copied()
    }

    pub fn longest_turn(&self) -> Option<i64> {
        self.turns.iter().max().copied()
    }

    /// Everything written, including subagents.
    pub fn output(&self) -> u64 {
        self.tokens.output + self.agent_output
    }
}

fn shorten(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max + 8));
    let mut n = 0;
    for (i, line) in s.trim().lines().enumerate() {
        if i > 0 {
            out.push_str(" ↵ ");
        }
        for ch in line.chars() {
            if n >= max {
                out.push('…');
                return out;
            }
            out.push(if ch.is_control() { ' ' } else { ch });
            n += 1;
        }
    }
    out
}

fn lines(s: &str) -> u32 {
    if s.is_empty() { 0 } else { s.lines().count() as u32 }
}

/// A call's detail, file and changed lines, from the tool's input.
fn describe(name: &str, input: &Value) -> (String, Option<String>, u32, u32) {
    let s = |k: &str| input[k].as_str().filter(|v| !v.is_empty());
    let path = s("file_path").or(s("notebook_path")).map(str::to_string);
    let (added, removed) = match name {
        "Edit" => (lines(s("new_string").unwrap_or("")), lines(s("old_string").unwrap_or(""))),
        "Write" => (lines(s("content").unwrap_or("")), 0),
        "NotebookEdit" => (lines(s("new_source").unwrap_or("")), 0),
        "MultiEdit" => input["edits"].as_array().into_iter().flatten().fold((0, 0), |(a, r), e| {
            (a + lines(e["new_string"].as_str().unwrap_or("")), r + lines(e["old_string"].as_str().unwrap_or("")))
        }),
        _ => (0, 0),
    };
    let detail = match name {
        "Agent" | "Task" => match (s("subagent_type"), s("description")) {
            (Some(t), Some(d)) => format!("{t}: {d}"),
            (t, d) => d.or(t).unwrap_or_default().to_string(),
        },
        "Grep" | "Glob" => match (s("pattern"), s("path")) {
            (Some(p), Some(dir)) => format!("{p}  in {dir}"),
            (p, _) => p.unwrap_or_default().to_string(),
        },
        _ => ["command", "file_path", "notebook_path", "url", "query", "pattern", "description", "skill", "prompt"]
            .iter()
            .find_map(|k| s(k))
            .map(str::to_string)
            .or_else(|| input.as_object().and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string))))
            .unwrap_or_default(),
    };
    (shorten(&detail, DETAIL_CHARS), path, added, removed)
}

impl Stats {
    fn seen(&mut self, at: i64) {
        self.first_at = Some(self.first_at.map_or(at, |f| f.min(at)));
        self.last_at = Some(self.last_at.map_or(at, |l| l.max(at)));
    }

    /// The line's time, or the last known one for lines without.
    fn time(&mut self, v: &Value) -> i64 {
        match v["timestamp"].as_str().and_then(crate::timefmt::parse_rfc3339) {
            Some(at) => {
                self.seen(at);
                at
            }
            None => self.last_at.unwrap_or(0),
        }
    }

    /// Takes in one transcript line.
    pub fn line(&mut self, line: &str) {
        if line.contains("\"type\":\"attachment\"") {
            // Sizes count until the first reply: that is the starting context.
            self.setup.line(line, self.replies.is_empty());
        } else if line.contains("\"type\":\"fork-context-ref\"") {
            self.fork_head = true;
        } else if line.contains("\"type\":\"assistant\"") {
            self.reply_line(line);
        } else if line.contains("\"type\":\"user\"") {
            self.user_line(line);
        } else if line.contains("\"compact_boundary\"") {
            self.compaction_line(line);
        } else if line.contains("\"type\":\"custom-title\"") || line.contains("\"type\":\"ai-title\"") {
            self.title_line(line);
        }
    }

    /// A reply. In a fork's transcript, the one before its first prompt is the
    /// reply of the session that started the fork: counted there.
    fn reply_line(&mut self, line: &str) {
        if self.fork_head {
            return;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            self.assistant(&v);
        }
    }

    /// A prompt, or tools' results.
    fn user_line(&mut self, line: &str) {
        self.fork_head = false;
        if !line.contains("\"tool_use_id\"") {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                self.prompt(&v);
            }
        } else if line.contains("\"is_error\":true") {
            // A tool result is often huge, so only failed ones are parsed.
            match (line.len() <= MAX_RESULT_LINE).then(|| serde_json::from_str::<Value>(line).ok()).flatten() {
                Some(v) => self.tool_results(&v),
                None => self.orphan_errors += 1,
            }
        } else {
            // Answered without an error: no need to remember the call's id.
            for rest in line.split("\"tool_use_id\":\"").skip(1) {
                if let Some(id) = rest.split('"').next() {
                    self.open.remove(id);
                }
            }
        }
    }

    /// The conversation was compacted.
    fn compaction_line(&mut self, line: &str) {
        let v = serde_json::from_str::<Value>(line).unwrap_or_default();
        // Written again later: counted once, by the line's id.
        if v["uuid"].as_str().is_some_and(|id| !self.counted.insert(crate::transcripts::once_key(b'k', id))) {
            return;
        }
        let at = if v.is_null() { self.last_at.unwrap_or(0) } else { self.time(&v) };
        self.compactions.push(at);
    }

    /// The session's title: one the user set wins over generated ones.
    fn title_line(&mut self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let custom = v["type"] == "custom-title";
        let title = v[if custom { "customTitle" } else { "aiTitle" }].as_str().map(str::trim);
        if let Some(title) = title.filter(|t| !t.is_empty() && (custom || !self.custom_title)) {
            self.title = Some(title.to_string());
            self.custom_title |= custom;
        }
    }

    /// One line of a Copilot session's `events.jsonl` (see `copilot`), read
    /// into the same numbers as a Claude Code transcript: prompts, a reply per
    /// model request with its tokens, tool calls and their errors.
    pub(crate) fn copilot_line(&mut self, line: &str) {
        const WANTED: [&str; 5] = [
            "session.start",
            "user.message",
            "session.usage_record",
            "tool.execution_start",
            "tool.execution_complete",
        ];
        if crate::copilot::kind_of(line).is_some_and(|k| !WANTED.contains(&k)) {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let data = &v["data"];
        match v["type"].as_str().unwrap_or_default() {
            "session.start" => {
                self.time(&v);
                if self.cwd.is_none() {
                    self.cwd = data["context"]["cwd"].as_str().filter(|c| !c.is_empty()).map(str::to_string);
                }
            }
            "user.message" => {
                let Some(text) = data["content"].as_str().filter(|t| !t.trim().is_empty()) else { return };
                if self.first_prompt.is_none() {
                    self.first_prompt = Some(shorten(text, 120));
                }
                let at = self.time(&v);
                self.prompts.push(Prompt { at, text: shorten(text, 140) });
            }
            "session.usage_record" => {
                let u = &data["usage"];
                if !u.is_object() {
                    return;
                }
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                // Copilot's input includes what came from the cache and what was written to it.
                let (cache_read, cache_write) = (n("cacheReadTokens"), n("cacheWriteTokens"));
                let tokens = Tokens {
                    input: n("inputTokens").saturating_sub(cache_read + cache_write),
                    cache_read,
                    cache_write,
                    cache_write_1h: 0,
                    output: n("outputTokens"),
                    thinking: n("reasoningTokens"),
                };
                let name = u["modelDisplayName"].as_str().or(u["model"].as_str()).filter(|m| !m.is_empty());
                let model = name.map(|name| match self.models.iter().position(|m| m == name) {
                    Some(i) => i as u16,
                    None => {
                        self.models.push(name.to_string());
                        (self.models.len() - 1) as u16
                    }
                });
                let at = self.time(&v);
                self.replies.push(Reply { at, model, tokens, web: 0 });
            }
            "tool.execution_start" => {
                let Some(name) = data["toolName"].as_str() else { return };
                let input = &data["arguments"];
                let s = |k: &str| input[k].as_str().filter(|v| !v.is_empty());
                let path = s("path").or(s("file_path")).map(str::to_string);
                let edits = EDIT_TOOLS.contains(&name);
                let added =
                    if edits { lines(s("new_str").or(s("file_text")).or(s("content")).unwrap_or("")) } else { 0 };
                let removed = if edits { lines(s("old_str").unwrap_or("")) } else { 0 };
                let detail =
                    ["command", "path", "file_path", "pattern", "query", "url", "question", "prompt", "description"]
                        .iter()
                        .find_map(|k| s(k))
                        .map(|d| shorten(d, DETAIL_CHARS))
                        .unwrap_or_default();
                let at = self.time(&v);
                if let Some(id) = data["toolCallId"].as_str() {
                    if !self.counted.insert(crate::transcripts::once_key(b'c', id)) {
                        return;
                    }
                    self.open.insert(id.to_string(), self.calls.len());
                }
                self.calls.push(Call { at, tool: name.to_string(), detail, path, added, removed, error: None });
            }
            "tool.execution_complete" => {
                self.time(&v);
                let Some(i) = data["toolCallId"].as_str().and_then(|id| self.open.remove(id)) else { return };
                if data["success"] == false {
                    let e = &data["error"];
                    let text = e["message"].as_str().or(e.as_str()).unwrap_or("failed");
                    match self.calls.get_mut(i) {
                        Some(call) => call.error = Some(shorten(text, ERROR_CHARS)),
                        None => self.orphan_errors += 1,
                    }
                }
            }
            _ => {}
        }
    }

    fn note_cwd(&mut self, v: &Value) {
        if self.cwd.is_none() {
            self.cwd = v["cwd"].as_str().filter(|c| !c.is_empty()).map(str::to_string);
        }
    }

    fn assistant(&mut self, v: &Value) {
        if v["type"] != "assistant" {
            return;
        }
        self.note_cwd(v);
        let at = self.time(v);
        let m = &v["message"];
        for item in m["content"].as_array().into_iter().flatten() {
            if item["type"] != "tool_use" {
                continue;
            }
            let name = item["name"].as_str().unwrap_or("?");
            let (detail, path, added, removed) = describe(name, &item["input"]);
            if let Some(id) = item["id"].as_str() {
                if !self.counted.insert(crate::transcripts::once_key(b'c', id)) {
                    continue;
                }
                self.open.insert(id.to_string(), self.calls.len());
            }
            self.calls.push(Call { at, tool: name.to_string(), detail, path, added, removed, error: None });
        }
        // One reply is written as several lines carrying the same usage, and
        // lines can be written again later: each reply counts once.
        if m["id"].as_str().is_some_and(|id| !self.counted.insert(crate::transcripts::once_key(b'm', id))) {
            return;
        }
        let u = &m["usage"];
        if !u.is_object() {
            return;
        }
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let tokens = Tokens {
            input: n("input_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
            cache_write_1h: u["cache_creation"]["ephemeral_1h_input_tokens"].as_u64().unwrap_or(0),
            output: n("output_tokens"),
            thinking: u["output_tokens_details"]["thinking_tokens"].as_u64().unwrap_or(0),
        };
        let web = (u["server_tool_use"]["web_search_requests"].as_u64().unwrap_or(0)
            + u["server_tool_use"]["web_fetch_requests"].as_u64().unwrap_or(0)) as u32;
        let model = m["model"].as_str().filter(|s| s.starts_with("claude")).map(|name| {
            match self.models.iter().position(|m| m == name) {
                Some(i) => i as u16,
                None => {
                    self.models.push(name.to_string());
                    (self.models.len() - 1) as u16
                }
            }
        });
        self.replies.push(Reply { at, model, tokens, web });
    }

    fn tool_results(&mut self, v: &Value) {
        for item in v["message"]["content"].as_array().into_iter().flatten() {
            if item["is_error"] != true {
                continue;
            }
            let text = match &item["content"] {
                Value::String(s) => s.clone(),
                Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
                _ => String::new(),
            };
            let text = text.trim();
            let error = shorten(text.strip_prefix("<tool_use_error>").unwrap_or(text), ERROR_CHARS);
            let error = error.trim_end_matches("</tool_use_error>").to_string();
            // A result written again: its call has its error already.
            let id = item["tool_use_id"].as_str();
            if id.is_some_and(|id| !self.counted.insert(crate::transcripts::once_key(b'e', id))) {
                continue;
            }
            // (An index that isn't a call's comes from a damaged kept file.)
            match id.and_then(|id| self.open.remove(id)).and_then(|i| self.calls.get_mut(i)) {
                Some(call) => call.error = Some(error),
                None => self.orphan_errors += 1,
            }
        }
    }

    fn prompt(&mut self, v: &Value) {
        // Not prompts: system notes, subagent prompts, and the summary that
        // continues a conversation after compaction.
        if v["type"] != "user"
            || v["isMeta"] == true
            || v["isSidechain"] == true
            || v["isCompactSummary"] == true
            || v["isVisibleInTranscriptOnly"] == true
        {
            return;
        }
        self.note_cwd(v);
        // Slash-command plumbing, system notes, interruptions and other sessions' messages aren't prompts.
        let Some(text) = crate::transcripts::user_prompt(v) else { return };
        if v["uuid"].as_str().is_some_and(|id| !self.counted.insert(crate::transcripts::once_key(b'p', id))) {
            return;
        }
        if self.first_prompt.is_none() {
            self.first_prompt = Some(shorten(text, 120));
        }
        let at = self.time(v);
        self.prompts.push(Prompt { at, text: shorten(text, 140) });
    }

    /// Sums of everything in `from..to` (unix seconds).
    pub fn summary(&self, from: i64, to: i64) -> Summary {
        let inside = |at: i64| at >= from && at < to;
        let mut s = Summary {
            prompts: self.prompts.iter().filter(|p| inside(p.at)).count() as u32,
            turns: self.turns().into_iter().filter(|t| inside(t.at)).map(|t| t.secs).collect(),
            compactions: self.compactions.iter().filter(|&&t| inside(t)).count() as u32,
            ..Default::default()
        };
        for r in self.replies.iter().filter(|r| inside(r.at)) {
            s.replies += 1;
            s.tokens.add(&r.tokens);
            s.web += r.web;
            s.cost += self.reply_cost(r);
        }
        let mut changed = HashSet::new();
        let mut read = HashSet::new();
        for c in self.calls.iter().filter(|c| inside(c.at)) {
            s.tool_calls += 1;
            if c.declined() {
                s.declined += 1;
            } else if c.error.is_some() {
                s.errors += 1;
            }
            s.agents += u32::from(c.agent());
            s.web += u32::from(c.web());
            s.commits += u32::from(c.commit() && c.error.is_none());
            // A call that failed or was declined changed and read nothing.
            if c.error.is_some() {
                continue;
            }
            s.added += u64::from(c.added);
            s.removed += u64::from(c.removed);
            if let Some(p) = &c.path {
                if c.edits() {
                    changed.insert(p.as_str());
                } else if c.tool == "Read" {
                    read.insert(p.as_str());
                }
            }
        }
        if from <= self.first_at.unwrap_or(0) && to > self.last_at.unwrap_or(0) {
            s.errors += self.orphan_errors;
        }
        s.files_changed = changed.len() as u32;
        s.files_read = read.len() as u32;
        s
    }

    pub fn reply_cost(&self, r: &Reply) -> f64 {
        self.model(r).map_or(0.0, |m| cost(m, &r.tokens))
    }

    /// Every turn: from a prompt to the last reply or tool call before the next.
    pub fn turns(&self) -> Vec<Turn> {
        let mut times: Vec<i64> = self.replies.iter().map(|r| r.at).chain(self.calls.iter().map(|c| c.at)).collect();
        times.sort_unstable();
        self.prompts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let next = self.prompts.get(i + 1).map_or(i64::MAX, |n| n.at);
                // The last activity before the next prompt, unless a long pause
                // came first: later activity was set off by something else, such
                // as a background task reporting back.
                let mut last = p.at;
                for &t in times.iter().skip(times.partition_point(|&t| t < p.at)) {
                    if t >= next || t - last > TURN_PAUSE {
                        break;
                    }
                    last = t;
                }
                Turn { at: p.at, secs: last - p.at, prompt: p.text.clone() }
            })
            .collect()
    }

    pub fn model(&self, r: &Reply) -> Option<&str> {
        r.model.and_then(|i| self.models.get(usize::from(i))).map(String::as_str)
    }

    /// The model that wrote the most.
    pub fn main_model(&self) -> Option<String> {
        let mut by: HashMap<&str, u64> = HashMap::new();
        for r in &self.replies {
            if let Some(m) = self.model(r) {
                *by.entry(m).or_default() += r.tokens.output;
            }
        }
        by.into_iter().max_by_key(|(_, v)| *v).map(|(k, _)| crate::transcripts::model_name(k))
    }

    /// The context of the first reply: what the session started with (tokens).
    pub fn starting_context(&self) -> Option<u64> {
        self.replies.first().map(|r| r.tokens.context()).filter(|&c| c > 0)
    }

    /// A title for lists: the session's title or its first prompt.
    pub fn display_title(&self) -> String {
        self.title.clone().or_else(|| self.first_prompt.clone()).unwrap_or_else(|| "Untitled session".into())
    }
}

// ---------- reading files, with a cache ----------

#[derive(Default)]
struct FileState {
    tail: crate::jsonl::Tail,
    stats: Arc<Stats>,
    /// What `keep` has of this session on disk, as far as this run knows.
    kept: Option<KeptState>,
}

/// How much of a session is in its kept file.
#[derive(Clone, Default, PartialEq)]
struct KeptState {
    /// Bytes of the transcript.
    offset: u64,
    /// Its subagents' transcripts.
    agents: Vec<AgentFile>,
}

/// A subagent's transcript as it was when it was read: name, modification time, size.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentFile {
    pub name: String,
    pub written: i64,
    pub size: u64,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, FileState>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, FileState>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Subagents' replies as kept statistics have them (tokens and models: what they
/// add to their session), by transcript: used while the transcript is as it was then.
fn kept_agents() -> &'static Mutex<KeptAgents> {
    static CACHE: OnceLock<Mutex<KeptAgents>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

type KeptAgents = HashMap<PathBuf, (AgentFile, Arc<Stats>)>;

/// Releases everything cached, e.g. when the statistics haven't been looked
/// at for a while. The next read starts from what was kept, or from scratch.
pub fn forget() {
    cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    kept_agents().lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    sizes().lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    crate::keep::forget();
    crate::os::trim_heap();
}

/// Statistics of one transcript, reading only what was appended since last time.
pub fn of_file(path: &Path) -> Option<Arc<Stats>> {
    read_file(path, None).map(|(stats, _, _)| stats)
}

/// [`of_file`], with how far the transcript is read now (bytes of whole lines)
/// and what is kept of it, as far as this run knows. With `session` (where it
/// runs, and its id), a transcript not read in this run starts from its kept
/// statistics, if they fit it.
fn read_file(path: &Path, session: Option<(&Source, &str)>) -> Option<(Arc<Stats>, u64, Option<KeptState>)> {
    let mut cache = cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((source, id)) = session.filter(|_| !cache.contains_key(path)) {
        if let Some(state) = from_kept(source, id, path) {
            cache.insert(path.to_path_buf(), state);
        }
    }
    let st = cache.entry(path.to_path_buf()).or_default();
    read_into(st, path)?;
    if is_copilot(path) {
        // Its name isn't in the events: it's in the file next to them.
        let name = crate::copilot::name_of(path).map(|n| shorten(&n, 140));
        if name.is_some() && st.stats.title != name {
            Arc::make_mut(&mut st.stats).title = name;
        }
    }
    Some((st.stats.clone(), st.tail.lines_end(), st.kept.clone()))
}

/// Whether `path` is a Copilot session's events (not a Claude Code transcript).
fn is_copilot(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "events.jsonl")
}

impl Stats {
    /// Only the replies and their models: what a subagent adds to its session (for `keep`).
    pub fn replies_only(&self) -> Stats {
        Stats { replies: self.replies.clone(), models: self.models.clone(), ..Stats::default() }
    }
}

/// Reads what was appended to a transcript since `st` last read it.
fn read_into(st: &mut FileState, path: &Path) -> Option<()> {
    use crate::jsonl::Outcome;
    let copilot = is_copilot(path);
    loop {
        let FileState { tail, stats, .. } = st;
        let outcome = tail.read(path, None, |line| {
            // Its own copy only once there is something to add.
            let stats = Arc::make_mut(stats);
            if copilot { stats.copilot_line(line) } else { stats.line(line) }
        });
        match outcome {
            Outcome::Replaced => *st = FileState::default(),
            Outcome::Unreadable => return None,
            Outcome::UpToDate | Outcome::More => return Some(()),
        }
    }
}

/// The transcript files of the subagents of the session whose transcript is `path`.
pub fn subagent_files(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(path.with_extension("").join("subagents")) else { return Vec::new() };
    entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect()
}

/// The subagents' transcripts of the session whose transcript is `path`, as they are now, by name.
fn agent_files(path: &Path) -> Vec<(PathBuf, AgentFile)> {
    let Ok(entries) = std::fs::read_dir(path.with_extension("").join("subagents")) else { return Vec::new() };
    let mut files: Vec<(PathBuf, AgentFile)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            Some((e.path(), AgentFile { name, written: modified(&meta), size: meta.len() }))
        })
        .collect();
    files.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    files
}

/// What the subagents of the session whose transcript is `path` add to it: their
/// replies. From what was kept for a transcript that is as it was then, else
/// from the transcript (only what was appended, like any other).
fn agent_replies(path: &Path) -> Vec<(AgentFile, Arc<Stats>)> {
    agent_files(path)
        .into_iter()
        .filter_map(|(file, now)| {
            let mut kept = kept_agents().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let as_kept = kept.get(&file).filter(|(then, _)| *then == now).map(|(_, stats)| stats.clone());
            if as_kept.is_none() {
                kept.remove(&file);
            }
            drop(kept);
            Some((now, as_kept.or_else(|| of_file(&file))?))
        })
        .collect()
}

/// When a session was last written: its transcript or one of its subagents'
/// (unix seconds). Kept statistics carry this time.
pub fn stamp(path: &Path) -> Option<i64> {
    let main = modified(&std::fs::metadata(path).ok()?);
    Some(agent_files(path).iter().map(|(_, f)| f.written).fold(main, i64::max))
}

/// A check of the bytes right before `offset` in a transcript (up to 256 of
/// them): reading goes on from what was kept only where the file still says
/// what it said then. `None` at the start of a file, or if it can't be read.
pub fn mark(path: &Path, offset: u64) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let n = offset.min(256);
    if n == 0 {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset - n)).ok()?;
    let mut bytes = vec![0u8; n as usize];
    file.read_exact(&mut bytes).ok()?;
    // FNV-1a.
    Some(bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h: u64, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)))
}

/// Where reading the transcript `path` of session `id` goes on: its kept
/// statistics, if they fit the transcript (the same bytes before where reading
/// stopped). Its subagents' kept replies go where [`agent_replies`] finds them.
fn from_kept(source: &Source, id: &str, path: &Path) -> Option<FileState> {
    let saved = crate::keep::resume(source, id).filter(|s| mark(path, s.offset) == Some(s.mark))?;
    let dir = path.with_extension("").join("subagents");
    let mut files = Vec::new();
    let mut agents = kept_agents().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for (file, stats) in saved.agents {
        files.push(file.clone());
        agents.insert(dir.join(&file.name), (file, Arc::new(stats)));
    }
    Some(FileState {
        tail: crate::jsonl::Tail::at(saved.offset),
        stats: Arc::new(saved.stats),
        kept: Some(KeptState { offset: saved.offset, agents: files }),
    })
}

/// A session's statistics and its subagents' replies.
type Loaded = (Arc<Stats>, Vec<Arc<Stats>>);

/// The statistics of the session `id` of `source` whose transcript is `path`,
/// and its subagents' replies. Read from the transcripts, but only what the
/// cache doesn't have yet; a session not read in this run starts from its kept
/// statistics, where they fit the transcript, so only what was written since is
/// read. What's kept is brought up to date afterwards: in the statistics once
/// enough is new (`exact` false), for `keep` whenever anything is.
///
/// Also returns whether there is anything to keep in it, and whether it was kept now.
fn read_session(source: &Source, id: &str, path: &Path, exact: bool) -> Option<(Loaded, bool, bool)> {
    if is_copilot(path) {
        return Some(((of_file(path)?, Vec::new()), false, false));
    }
    // Before reading: what's kept under this time has at least what was written until then.
    let written = std::fs::metadata(path).map_or(0, |m| modified(&m));
    let (stats, offset, kept) = read_file(path, Some((source, id)))?;
    let agents = agent_replies(path);
    let worth = !stats.prompts.is_empty() || !stats.replies.is_empty();
    let mut kept_now = false;
    if worth && crate::keep::enabled() {
        let now = KeptState { offset, agents: agents.iter().map(|(f, _)| f.clone()).collect() };
        let stamp = agents.iter().map(|(f, _)| f.written).fold(written, i64::max);
        let due = match &kept {
            None => true,
            Some(k) if exact => *k != now || crate::keep::written(source, id) != Some(stamp),
            Some(k) => {
                let new = now.agents.iter().filter(|f| !k.agents.contains(f)).map(|f| f.size).sum::<u64>();
                now.offset.saturating_sub(k.offset) + new >= KEEP_AFTER
            }
        };
        if due {
            if let Some(mark) = mark(path, now.offset) {
                let saved = crate::keep::Saving { stats: &stats, agents: &agents, offset: now.offset, mark };
                // A session may be written on within the same second: only `keep`, which
                // waits until it has been quiet for a while, says "everything until then".
                let as_of = if exact { stamp } else { stamp - 1 };
                if crate::keep::store(source, id, as_of, &saved) {
                    kept_now = true;
                    if let Some(st) = cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_mut(path) {
                        st.kept = Some(now);
                    }
                }
            }
        }
    }
    Some(((stats, agents.into_iter().map(|(_, s)| s).collect()), worth, kept_now))
}

/// For the statistics: see [`read_session`].
fn session_stats(source: &Source, id: &str, path: &Path) -> Option<Loaded> {
    read_session(source, id, path, false).map(|(loaded, _, _)| loaded)
}

/// For `keep`: brings what's kept of a session up to date, reading only what
/// its transcripts got since. `Some(false)`: nothing new, or nothing in it to
/// keep (`worth` says which). What it read doesn't stay in the cache, unless
/// the statistics had it there already.
pub fn keep_session(source: &Source, id: &str, path: &Path) -> Option<(bool, bool)> {
    let known = cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains_key(path);
    let read = read_session(source, id, path, true).map(|(_, worth, kept)| (kept, worth));
    if !known {
        let dir = path.with_extension("");
        cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|p, _| p != path && !p.starts_with(&dir));
        kept_agents().lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|p, _| !p.starts_with(&dir));
    }
    read
}

// ---------- progress ----------

/// How far reading for the statistics has got: sessions done, of how many.
#[derive(Default)]
pub struct Progress {
    pub done: std::sync::atomic::AtomicU32,
    pub total: std::sync::atomic::AtomicU32,
}

thread_local! {
    static PROGRESS: std::cell::RefCell<Option<Arc<Progress>>> = const { std::cell::RefCell::new(None) };
}

/// What this thread reads for the statistics from now on is told to `progress`.
pub fn report_to(progress: Arc<Progress>) {
    PROGRESS.with(|p| *p.borrow_mut() = Some(progress));
}

fn progress(tell: impl FnOnce(&Progress)) {
    PROGRESS.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            tell(p);
        }
    });
}

/// A subagent of a session, for the agent map.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRun {
    /// What it was asked to do, e.g. "Taiga UI variant".
    pub description: String,
    /// Its type, e.g. `fork`, `Explore`, `general-purpose`.
    pub kind: String,
    /// Not finished, and its transcript still being written.
    pub running: bool,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    pub model: Option<String>,
    pub output: u64,
    /// Its context at its last reply.
    pub context: u64,
    pub tool_calls: u32,
    pub errors: u32,
    /// At API prices (dollars).
    pub cost: f64,
}

/// A subagent whose transcript hasn't changed for this long isn't running,
/// whatever its metadata says (its session ended without finishing it).
const AGENT_STALE_SECS: i64 = 10 * 60;

/// What a subagent's `agent-<id>.meta.json` says: description, type, finished.
fn agent_meta(text: &str) -> (Option<String>, Option<String>, bool) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return (None, None, false) };
    let s = |k: &str| v[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    (s("description"), s("agentType"), v["finished"] == true)
}

/// The subagents of the session whose transcript is `path`, oldest first.
fn agent_runs(path: &Path, now: i64) -> Vec<AgentRun> {
    let mut runs: Vec<AgentRun> = subagent_files(path)
        .iter()
        .filter_map(|p| {
            let stats = of_file(p)?;
            let meta = std::fs::read_to_string(p.with_extension("meta.json")).unwrap_or_default();
            let (description, kind, finished) = agent_meta(&meta);
            let written = std::fs::metadata(p).ok().map(|m| modified(&m)).unwrap_or(0);
            let sum = stats.summary(i64::MIN, i64::MAX);
            Some(AgentRun {
                description: description.unwrap_or_else(|| stats.display_title()),
                kind: kind.unwrap_or_default(),
                running: !finished && now - written < AGENT_STALE_SECS,
                first_at: stats.first_at,
                last_at: stats.last_at,
                model: stats.main_model(),
                output: sum.tokens.output,
                context: stats.replies.last().map_or(0, |r| r.tokens.context()),
                tool_calls: sum.tool_calls,
                errors: sum.errors,
                cost: sum.cost,
            })
        })
        .collect();
    runs.sort_by_key(|r| r.first_at);
    runs
}

/// Output tokens and API cost of subagents in `from..to`.
fn agent_work(agents: &[Arc<Stats>], from: i64, to: i64) -> (u64, f64) {
    let mut out = (0, 0.0);
    for a in agents {
        for r in a.replies.iter().filter(|r| r.at >= from && r.at < to) {
            out.0 += r.tokens.output;
            out.1 += a.reply_cost(r);
        }
    }
    out
}

// ---------- disk usage, with a cache ----------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub bytes: u64,
    pub files: u64,
}

impl Size {
    fn add(&mut self, o: Size) {
        self.bytes += o.bytes;
        self.files += o.files;
    }
}

/// How long folder sizes are reused.
const SIZE_TTL: Duration = Duration::from_secs(5 * 60);

fn walk(path: &Path) -> Size {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return Size::default() };
    if meta.is_file() {
        return Size { bytes: meta.len(), files: 1 };
    }
    let mut total = Size::default();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file() {
                total.add(Size { bytes: e.metadata().map_or(0, |m| m.len()), files: 1 });
            }
        }
    }
    total
}

/// Size of a file or folder (folders cached for a few minutes).
fn sizes() -> &'static Mutex<HashMap<PathBuf, (Instant, Size)>> {
    static SIZES: OnceLock<Mutex<HashMap<PathBuf, (Instant, Size)>>> = OnceLock::new();
    SIZES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn size_of(path: &Path) -> Size {
    let sizes = sizes();
    if let Some((at, size)) = sizes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(path) {
        if at.elapsed() < SIZE_TTL {
            return *size;
        }
    }
    let size = walk(path);
    sizes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(path.to_path_buf(), (Instant::now(), size));
    size
}

/// A file or folder and its size.
#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub label: String,
    pub path: PathBuf,
    pub size: Size,
    /// A folder (looked up here, in the background, not while drawing).
    pub dir: bool,
}

impl Usage {
    fn new(label: impl Into<String>, path: PathBuf, size: Size) -> Self {
        let dir = path.is_dir();
        Self { label: label.into(), path, size, dir }
    }
}

// ---------- finding transcripts ----------

fn modified(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// A session's transcript, or what was kept of it once Claude Code deleted it.
struct Found<'a> {
    source: &'a Source,
    path: PathBuf,
    id: String,
    /// `path` is kept statistics (`keep`), not a transcript.
    kept: bool,
    /// What it says, once read.
    loaded: std::cell::OnceCell<Option<Loaded>>,
}

impl<'a> Found<'a> {
    fn new(source: &'a Source, path: PathBuf, id: String, kept: bool) -> Self {
        Found { source, path, id, kept, loaded: std::cell::OnceCell::new() }
    }

    fn load(&self) -> Option<&Loaded> {
        self.loaded
            .get_or_init(|| {
                let loaded = if self.kept {
                    crate::keep::load(&self.path)
                } else {
                    session_stats(self.source, &self.id, &self.path)
                };
                progress(|p| {
                    p.done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                });
                loaded
            })
            .as_ref()
    }

    fn stats(&self) -> Option<Arc<Stats>> {
        self.load().map(|(s, _)| s.clone())
    }

    fn agents(&self) -> Vec<Arc<Stats>> {
        self.load().map(|(_, a)| a.clone()).unwrap_or_default()
    }
}

/// The transcript files of `source` written to since `since`, with when: every
/// `.jsonl` in a project's folder. (No projects folder left can still mean kept
/// statistics: then there are none here.)
fn transcripts_in(source: &Source, since: i64) -> Vec<(i64, PathBuf)> {
    let folders = std::fs::read_dir(source.config_dir.join("projects")).into_iter().flatten().flatten();
    folders
        .filter_map(|folder| std::fs::read_dir(folder.path()).ok())
        .flat_map(Iterator::flatten)
        .filter(|f| f.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|f| (f.metadata().map_or(0, |m| modified(&m)), f.path()))
        .filter(|(written, _)| since <= 0 || *written >= since)
        .collect()
}

/// Every session transcript in `roots` written to since `since`.
///
/// A session can have copies in several project folders (when its project
/// moved, Claude Code keeps the old one): only the latest written counts.
fn transcripts(roots: &[Source], since: i64) -> Vec<Found<'_>> {
    // By place (Windows, or a distribution, whose users share what's kept) and id.
    let mut by_id: HashMap<(Option<&str>, String), (i64, Found<'_>)> = HashMap::new();
    for source in roots {
        for (written, path) in transcripts_in(source, since) {
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let key = (source.distro.as_deref(), id.clone());
            if by_id.get(&key).is_none_or(|(t, _)| written > *t) {
                by_id.insert(key, (written, Found::new(source, path, id, false)));
            }
        }
    }
    // Sessions whose transcript Claude Code deleted (or whose distribution isn't
    // running), from what was kept: after all transcripts, which come first.
    let mut places = HashSet::new();
    for source in roots.iter().filter(|s| places.insert(s.distro.as_deref())) {
        for (id, path, written) in crate::keep::list(source) {
            if since == 0 || written >= since {
                let key = (source.distro.as_deref(), id.clone());
                by_id.entry(key).or_insert((written, Found::new(source, path, id, true)));
            }
        }
    }
    // Copilot's sessions (while it's switched on), with the Windows ones.
    if let Some(source) = roots.iter().find(|s| s.distro.is_none()) {
        for (id, path) in crate::copilot::event_files() {
            let written = std::fs::metadata(&path).map_or(0, |m| modified(&m));
            if since == 0 || written >= since {
                by_id.insert((None, id.clone()), (written, Found::new(source, path, id, false)));
            }
        }
    }
    progress(|p| {
        p.done.store(0, std::sync::atomic::Ordering::Relaxed);
        p.total.store(by_id.len() as u32, std::sync::atomic::Ordering::Relaxed);
    });
    by_id.into_values().map(|(_, f)| f).collect()
}

/// The transcripts of one place (the latest copy of each): id and file.
pub fn transcript_files(source: &Source) -> Vec<(String, PathBuf)> {
    // Claude Code's only: these are the ones `keep` keeps statistics of.
    let claude = |f: &Found| !f.kept && !f.id.starts_with(crate::copilot::ID_PREFIX);
    transcripts(std::slice::from_ref(source), 0).into_iter().filter(claude).map(|f| (f.id, f.path)).collect()
}

/// Where the transcript of session `id` is.
pub fn locate(roots: &[Source], id: &str) -> Option<(Source, PathBuf)> {
    if id.starts_with(crate::copilot::ID_PREFIX) {
        let source = roots.iter().find(|s| s.distro.is_none())?;
        return Some((source.clone(), crate::copilot::events_of(id)?));
    }
    let name = format!("{id}.jsonl");
    for source in roots {
        let Ok(folders) = std::fs::read_dir(source.config_dir.join("projects")) else { continue };
        // The latest written copy, if the project moved.
        let newest = folders
            .flatten()
            .map(|folder| folder.path().join(&name))
            .filter_map(|path| {
                std::fs::metadata(&path).ok().filter(std::fs::Metadata::is_file).map(|m| (modified(&m), path))
            })
            .max_by_key(|(t, _)| *t);
        if let Some((_, path)) = newest {
            return Some((source.clone(), path));
        }
    }
    None
}

pub fn project_name(cwd: &str) -> String {
    cwd.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(cwd).to_string()
}

// ---------- one session ----------

/// A session whose transcript is gone, from its kept statistics.
fn kept_session(roots: &[Source], id: &str) -> Option<SessionData> {
    let (source, path) = roots
        .iter()
        .find_map(|s| crate::keep::list(s).into_iter().find(|(i, _, _)| i == id).map(|(_, p, _)| (s, p)))?;
    let (stats, agents) = crate::keep::load(&path)?;
    let mut summary = stats.summary(i64::MIN, i64::MAX);
    (summary.agent_output, summary.agent_cost) = agent_work(&agents, i64::MIN, i64::MAX);
    let context = stats.replies.iter().map(|r| (r.at, r.tokens.context())).collect();
    let storage = vec![Usage::new("Kept statistics", path.clone(), size_of(&path))];
    Some(SessionData {
        id: id.to_string(),
        distro: source.distro.clone(),
        transcript: path,
        kept: true,
        summary,
        context,
        storage,
        memory_files: None,
        agents: Vec::new(),
        models: models_of(&stats, &agents),
        stats,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionData {
    pub id: String,
    pub distro: Option<String>,
    pub transcript: PathBuf,
    /// The transcript is gone; these are kept statistics.
    pub kept: bool,
    pub stats: Arc<Stats>,
    pub summary: Summary,
    /// Context size at each reply: (unix seconds, tokens).
    pub context: Vec<(i64, u64)>,
    /// Files and folders that belong to the session.
    pub storage: Vec<Usage>,
    /// Memory files of the session's project, if its folder is known.
    pub memory_files: Option<u32>,
    /// Its subagents, oldest first (none for kept statistics).
    pub agents: Vec<AgentRun>,
    /// The models that wrote its replies and its subagents', the one that wrote the most first.
    pub models: Vec<ModelUse>,
}

/// Statistics of session `id`, including what its subagents wrote and the
/// space its files take.
pub fn session(roots: &[Source], id: &str) -> Option<SessionData> {
    let Some((source, path)) = locate(roots, id) else { return kept_session(roots, id) };
    // Its numbers, and its subagents' replies.
    let (stats, replies) = session_stats(&source, id, &path)?;
    let mut summary = stats.summary(i64::MIN, i64::MAX);
    (summary.agent_output, summary.agent_cost) = agent_work(&replies, i64::MIN, i64::MAX);
    let context = stats.replies.iter().map(|r| (r.at, r.tokens.context())).collect();
    let mut storage = vec![Usage::new("Transcript", path.clone(), size_of(&path))];
    let dir = path.with_extension("");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let label = match e.file_name().to_string_lossy().as_ref() {
                "subagents" => "Subagent transcripts".to_string(),
                "tool-results" => "Saved tool output".to_string(),
                other => other.to_string(),
            };
            storage.push(Usage::new(label, e.path(), walk(&e.path())));
        }
    }
    for (label, folder) in [("File history", "file-history"), ("Session environment", "session-env")] {
        let p = source.config_dir.join(folder).join(id);
        let size = if p.is_dir() { walk(&p) } else { Size::default() };
        if size.files > 0 {
            storage.push(Usage::new(label, p, size));
        }
    }
    // Copilot's sessions have none (that's Claude Code's project memory).
    let memory_files = path.parent().filter(|_| !is_copilot(&path)).map(crate::setup::memory_files);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let agents = agent_runs(&path, now);
    Some(SessionData {
        id: id.to_string(),
        distro: source.distro.clone(),
        transcript: path,
        kept: false,
        models: models_of(&stats, &replies),
        stats,
        summary,
        context,
        storage,
        memory_files,
        agents,
    })
}

// ---------- all sessions ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Period {
    Today,
    Week,
    Month,
    All,
}

impl Period {
    pub const ALL: [Period; 4] = [Period::Today, Period::Week, Period::Month, Period::All];

    pub fn label(self) -> &'static str {
        match self {
            Period::Today => "Today",
            Period::Week => "7 days",
            Period::Month => "30 days",
            Period::All => "All time",
        }
    }

    /// Where the period starts (a local midnight), given the start of today.
    pub fn start(self, today: i64, offset: Offset) -> i64 {
        // From the middle of that day: the clocks may have changed since.
        let days_ago = |n: i64| day_start_at(today - n * DAY + DAY / 2, offset);
        match self {
            Period::Today => today,
            Period::Week => days_ago(6),
            Period::Month => days_ago(29),
            Period::All => 0,
        }
    }
}

/// The local time's offset from UTC (seconds) at a moment. It changes with
/// the clocks, twice a year: a statistic over weeks can't use today's.
pub type Offset<'a> = &'a dyn Fn(i64) -> i64;

/// The start of the local day containing `t`: its midnight, by the offset
/// that applied then (not `t`'s, when the clocks changed during the day).
pub fn day_start_at(t: i64, offset: Offset) -> i64 {
    let date = (t + offset(t)).div_euclid(DAY) * DAY;
    date - offset(date - offset(t))
}

/// The time a period's numbers (`from..to`) are compared with: as long a time
/// right before it; for today, yesterday from midnight until the same time of
/// day (the hours right before midnight would be yesterday's evening against
/// this morning). `None` for all time, which has nothing before it.
pub fn previous_range(period: Period, from: i64, to: i64, offset: Offset) -> Option<(i64, i64)> {
    let len = to - from;
    match period {
        Period::All => None,
        Period::Today => {
            let start = day_start_at(from - DAY / 2, offset);
            Some((start, (start + len).min(from)))
        }
        _ => Some((from - len, from)),
    }
}

/// Activity in one bucket of a chart.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Activity {
    pub prompts: u32,
    pub replies: u32,
    pub output: u64,
    pub agent_output: u64,
    pub tool_calls: u32,
    pub errors: u32,
    /// At API prices, subagents included (dollars).
    pub cost: f64,
}

/// One project's share.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub name: String,
    pub output: u64,
    pub sessions: u32,
}

/// How much one model was used: tokens it wrote and read, replies, cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelUse {
    pub name: String,
    pub output: u64,
    /// Read: input, cache reads and cache writes.
    pub input: u64,
    pub replies: u32,
    /// At API prices (dollars).
    pub cost: f64,
}

impl ModelUse {
    /// Counts reply `r` of `model` (as the transcript names it) in `models`; `cost` at API prices.
    fn count(models: &mut HashMap<String, ModelUse>, model: Option<&str>, r: &Reply, cost: f64) {
        let Some(model) = model else { return };
        let name = crate::transcripts::model_name(model);
        let u = models.entry(name.clone()).or_insert_with(|| ModelUse { name, ..Default::default() });
        u.output += r.tokens.output;
        u.input += r.tokens.input + r.tokens.cache_read + r.tokens.cache_write;
        u.replies += 1;
        u.cost += cost;
    }

    /// The models by tokens written, the most first.
    fn ranked(models: HashMap<String, ModelUse>) -> Vec<ModelUse> {
        let mut models: Vec<ModelUse> = models.into_values().collect();
        models.sort_by(|a, b| b.output.cmp(&a.output).then(a.name.cmp(&b.name)));
        models
    }
}

/// The models that wrote a session's replies and its subagents', the one that wrote the most first.
fn models_of(s: &Stats, agents: &[Arc<Stats>]) -> Vec<ModelUse> {
    let mut models = HashMap::new();
    for stats in std::iter::once(s).chain(agents.iter().map(Arc::as_ref)) {
        for r in &stats.replies {
            ModelUse::count(&mut models, stats.model(r), r, stats.reply_cost(r));
        }
    }
    ModelUse::ranked(models)
}

/// A project in the list of projects: its sessions summed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectInfo {
    pub name: String,
    pub sessions: u32,
    pub prompts: u32,
    pub output: u64,
    pub cost: f64,
    pub errors: u32,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
}

/// After a Copilot session's project in the list of all sessions.
const COPILOT_TAG: &str = " · Copilot";

/// The projects of these sessions, the latest used first. A project is its
/// folder, whichever agent worked in it.
pub fn projects(sessions: &[SessionInfo]) -> Vec<ProjectInfo> {
    let mut by_name: HashMap<&str, ProjectInfo> = HashMap::new();
    for s in sessions {
        let name = s.project.strip_suffix(COPILOT_TAG).unwrap_or(&s.project);
        let p = by_name.entry(name).or_insert_with(|| ProjectInfo { name: name.to_string(), ..Default::default() });
        p.sessions += 1;
        p.prompts += s.prompts;
        p.output += s.output;
        p.cost += s.cost;
        p.errors += s.errors;
        p.first_at = p.first_at.into_iter().chain(s.first_at).min();
        p.last_at = p.last_at.into_iter().chain(s.last_at).max();
    }
    let mut list: Vec<ProjectInfo> = by_name.into_values().collect();
    list.sort_by(|a, b| b.last_at.cmp(&a.last_at).then(a.name.cmp(&b.name)));
    list
}

/// A session in lists.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub project: String,
    pub distro: Option<String>,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    pub prompts: u32,
    pub output: u64,
    pub errors: u32,
    /// At API prices, subagents included (dollars).
    pub cost: f64,
    pub size: Size,
    pub archived: bool,
    /// Its transcript is gone: from kept statistics.
    pub kept: bool,
}

/// A turn in lists, with its session.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnRow {
    pub id: String,
    pub title: String,
    pub turn: Turn,
}

/// Failures that look alike.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub signature: String,
    pub count: u32,
    /// The latest one's detail and error.
    pub example: Call,
}

/// Groups failed calls by [`Call::signature`], most frequent first.
pub fn failures<'a>(calls: impl Iterator<Item = &'a Call>) -> Vec<Failure> {
    let mut by: HashMap<String, Failure> = HashMap::new();
    for c in calls {
        if let Some(sig) = c.signature() {
            let f = by.entry(sig.clone()).or_insert_with(|| Failure { signature: sig, count: 0, example: c.clone() });
            f.count += 1;
            if c.at >= f.example.at {
                f.example = c.clone();
            }
        }
    }
    let mut v: Vec<Failure> = by.into_values().collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then(a.signature.cmp(&b.signature)));
    v
}

/// All sessions in a period.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    pub from: i64,
    pub to: i64,
    pub sessions: u32,
    pub summary: Summary,
    /// Hours with any activity.
    pub active_hours: u32,
    /// Length of one chart bucket (an hour, a day or a week).
    pub bucket: i64,
    /// Chart buckets, oldest first: (start, activity).
    pub buckets: Vec<(i64, Activity)>,
    /// Prompts and replies by local weekday (Monday first) and hour.
    pub heat: Vec<[u32; 24]>,
    pub projects: Vec<Project>,
    pub tools: Vec<(String, u32)>,
    /// By tokens written, the most first; subagents included.
    pub models: Vec<ModelUse>,
    pub commands: Vec<(String, u32)>,
    /// The sessions that wrote the most.
    pub top: Vec<SessionInfo>,
    /// The longest turns.
    pub longest: Vec<TurnRow>,
    /// Failures that happened most often.
    pub failures: Vec<Failure>,
    /// The same sums for the time just before, as long as this period.
    pub previous: Option<Summary>,
    /// Sessions started in the period: (when, starting context in tokens).
    pub starts: Vec<(i64, u64)>,
    /// What the latest of them loaded, and its starting context.
    pub setup: Option<(crate::setup::Setup, u64)>,
    /// Memory files over all projects.
    pub memory_files: u32,
}

fn sorted<K: Clone + Ord, V: Copy + Ord>(map: &HashMap<K, V>) -> Vec<(K, V)> {
    let mut v: Vec<(K, V)> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// The programs a shell command runs, e.g. `git` and `cargo` for
/// `cd x && git status && cargo build`.
fn programs(command: &str) -> Vec<String> {
    // With a here-document, the lines after the first are its text, not commands.
    let first = command.split(" ↵ ").next().unwrap_or(command);
    let command = if first.contains("<<") { first } else { command };
    command
        .split("&&")
        .flat_map(|s| s.split("||"))
        // Not at pipes: `| head` and `| grep` would drown out the rest.
        .flat_map(|s| s.split([';', '↵']))
        .map(|s| s.split('|').next().unwrap_or(s))
        .filter_map(|part| {
            let word = part.split_whitespace().find(|w| !w.contains('='))?;
            let word = word.trim_matches(['"', '\'', '(', ')', '&', '{', '}']);
            let name = word.rsplit(['\\', '/']).next().unwrap_or(word);
            let name = name.strip_suffix(".exe").unwrap_or(name).to_ascii_lowercase();
            // Changing directories isn't what a command is about.
            let skip = name.is_empty()
                || matches!(name.as_str(), "cd" | "pushd" | "popd" | "set-location" | "sl" | "push-location")
                || !name.chars().next().is_some_and(char::is_alphanumeric);
            (!skip).then_some(name)
        })
        .collect()
}

/// Everything in `from..to` over all sessions in `roots`. `offset` places
/// hours and day boundaries in local time.
pub fn overview(roots: &[Source], from: i64, to: i64, offset: Offset, archived: &HashSet<String>) -> Overview {
    overview_of(roots, from, to, offset, archived, None)
}

/// [`overview`] of one project's sessions (by [`project_name`]), or of all.
pub fn overview_of(
    roots: &[Source],
    from: i64,
    to: i64,
    offset: Offset,
    archived: &HashSet<String>,
    project: Option<&str>,
) -> Overview {
    let mut tally = Tally::new(from, to, offset);
    for found in transcripts(roots, from) {
        let Some(s) = found.stats() else { continue };
        if s.last_at.is_none_or(|t| t < from) || s.first_at.is_some_and(|t| t >= to) {
            continue;
        }
        let name = s.cwd.as_deref().map(project_name).unwrap_or_else(|| "?".into());
        if project.is_some_and(|p| p != name) {
            continue;
        }
        let agents = found.agents();
        let mut sum = s.summary(from, to);
        (sum.agent_output, sum.agent_cost) = agent_work(&agents, from, to);
        if sum.prompts == 0 && sum.replies == 0 {
            continue;
        }
        tally.session(&s, &agents, &sum, &found.id);
        tally.top.push(SessionInfo {
            id: found.id.clone(),
            title: s.display_title(),
            project: name,
            distro: found.source.distro.clone(),
            first_at: s.first_at,
            last_at: s.last_at,
            prompts: sum.prompts,
            output: sum.output(),
            errors: sum.errors,
            cost: sum.total_cost(),
            size: Size::default(),
            archived: archived.contains(&found.id),
            kept: found.kept,
        });
    }
    let mut o = tally.finish();
    o.memory_files = memory_files(roots);
    o
}

/// Where a time is in its local week and day: (weekday, Monday first; hour).
fn weekday_and_hour(t: i64, offset: Offset) -> (usize, usize) {
    let l = t + offset(t);
    // 1970-01-01 was a Thursday: Monday is 0.
    ((l.div_euclid(DAY) + 3).rem_euclid(7) as usize, (l.rem_euclid(DAY) / HOUR) as usize)
}

/// What the overview adds up, session by session, before it is sorted and put
/// into the chart's buckets.
struct Tally<'a> {
    from: i64,
    to: i64,
    offset: Offset<'a>,
    o: Overview,
    /// By name: tokens written and sessions.
    projects: HashMap<String, (u64, u32)>,
    tools: HashMap<String, u32>,
    models: HashMap<String, ModelUse>,
    commands: HashMap<String, u32>,
    /// Files changed and read, each once over all sessions.
    files: HashSet<String>,
    read: HashSet<String>,
    /// Activity per local hour; the buckets are filled once the range is known.
    hourly: HashMap<i64, Activity>,
    top: Vec<SessionInfo>,
    longest: Vec<TurnRow>,
    failed: Vec<Call>,
    /// The latest session started in the period: when, what it loaded, its starting context.
    latest: Option<(i64, crate::setup::Setup, u64)>,
}

impl<'a> Tally<'a> {
    fn new(from: i64, to: i64, offset: Offset<'a>) -> Self {
        Tally {
            from,
            to,
            offset,
            o: Overview { from, to, heat: vec![[0; 24]; 7], ..Default::default() },
            projects: HashMap::new(),
            tools: HashMap::new(),
            models: HashMap::new(),
            commands: HashMap::new(),
            files: HashSet::new(),
            read: HashSet::new(),
            hourly: HashMap::new(),
            top: Vec::new(),
            longest: Vec::new(),
            failed: Vec::new(),
            latest: None,
        }
    }

    /// Whether a time is in the period (a check that doesn't hold on to the tally).
    fn inside(&self) -> impl Fn(i64) -> bool + use<> {
        let (from, to) = (self.from, self.to);
        move |at| at >= from && at < to
    }

    /// The activity of the local hour `t` is in (not UTC's hour: some time
    /// zones are half an hour off).
    fn hour(&mut self, t: i64) -> &mut Activity {
        let start = t - (t + (self.offset)(t)).rem_euclid(HOUR);
        self.hourly.entry(start).or_default()
    }

    /// One more prompt or reply at `t`, for "when you work".
    fn heat(&mut self, t: i64) {
        let (day, hour) = weekday_and_hour(t, self.offset);
        if let Some(cell) = self.o.heat.get_mut(day).and_then(|row| row.get_mut(hour)) {
            *cell += 1;
        }
    }

    fn use_model(&mut self, model: Option<&str>, r: &Reply, cost: f64) {
        ModelUse::count(&mut self.models, model, r, cost);
    }

    /// Adds a session: `sum` is its summary for the period, `agents` its subagents' replies.
    fn session(&mut self, s: &Stats, agents: &[Arc<Stats>], sum: &Summary, id: &str) {
        self.o.sessions += 1;
        self.o.summary.add(sum);
        self.start(s);
        let inside = self.inside();
        for p in s.prompts.iter().filter(|p| inside(p.at)) {
            self.hour(p.at).prompts += 1;
            self.heat(p.at);
        }
        self.turns(s, id);
        self.failed.extend(s.calls.iter().filter(|c| inside(c.at) && c.signature().is_some()).cloned());
        self.replies(s, agents);
        for c in s.calls.iter().filter(|c| inside(c.at)) {
            self.call(c);
        }
        let name = s.cwd.as_deref().map(project_name).unwrap_or_else(|| "?".into());
        let p = self.projects.entry(name).or_default();
        p.0 += sum.output();
        p.1 += 1;
    }

    /// A session that started in the period: its starting context, and what it
    /// loaded if it's the latest.
    fn start(&mut self, s: &Stats) {
        let inside = self.inside();
        let (Some(at), Some(context)) = (s.first_at.filter(|&t| inside(t)), s.starting_context()) else {
            return;
        };
        self.o.starts.push((at, context));
        if s.setup.known && self.latest.as_ref().is_none_or(|(t, _, _)| at > *t) {
            self.latest = Some((at, s.setup.clone(), context));
        }
    }

    /// The session's turns among the eight longest.
    fn turns(&mut self, s: &Stats, id: &str) {
        let (title, inside) = (s.display_title(), self.inside());
        let turns = s.turns().into_iter().filter(|t| inside(t.at) && t.secs > 0);
        self.longest.extend(turns.map(|turn| TurnRow { id: id.to_string(), title: title.clone(), turn }));
        self.longest.sort_by_key(|t| std::cmp::Reverse(t.turn.secs));
        self.longest.truncate(8);
    }

    /// The session's replies and its subagents'.
    fn replies(&mut self, s: &Stats, agents: &[Arc<Stats>]) {
        let inside = self.inside();
        for r in s.replies.iter().filter(|r| inside(r.at)) {
            let cost = s.reply_cost(r);
            let a = self.hour(r.at);
            a.replies += 1;
            a.output += r.tokens.output;
            a.cost += cost;
            self.heat(r.at);
            self.use_model(s.model(r), r, cost);
        }
        for agent in agents {
            for r in agent.replies.iter().filter(|r| inside(r.at)) {
                let cost = agent.reply_cost(r);
                let a = self.hour(r.at);
                a.agent_output += r.tokens.output;
                a.cost += cost;
                self.use_model(agent.model(r), r, cost);
            }
        }
    }

    /// One tool call: its hour, its tool, the programs of a shell command, its file.
    fn call(&mut self, c: &Call) {
        let a = self.hour(c.at);
        a.tool_calls += 1;
        a.errors += u32::from(c.error.is_some() && !c.declined());
        *self.tools.entry(c.tool.clone()).or_default() += 1;
        if c.shell() {
            for p in programs(&c.detail) {
                *self.commands.entry(p).or_default() += 1;
            }
        }
        match &c.path {
            Some(p) if c.edits() => drop(self.files.insert(p.clone())),
            Some(p) if c.tool == "Read" => drop(self.read.insert(p.clone())),
            _ => {}
        }
    }

    /// The overview: everything sorted, the best of each list, the chart's buckets.
    fn finish(self) -> Overview {
        let Tally { from, to, offset, mut o, projects, tools, models, commands, files, read, hourly, .. } = self;
        o.longest = self.longest;
        o.starts.sort_unstable();
        o.setup = self.latest.map(|(_, s, c)| (s, c));
        o.failures = failures(self.failed.iter());
        o.failures.truncate(8);
        // Distinct over all sessions, not summed per session.
        o.summary.files_changed = files.len() as u32;
        o.summary.files_read = read.len() as u32;
        o.active_hours = hourly.values().filter(|a| a.prompts + a.replies > 0).count() as u32;
        (o.bucket, o.buckets) = buckets(hourly, from, to, offset);
        o.tools = sorted(&tools);
        o.models = ModelUse::ranked(models);
        o.commands = sorted(&commands);
        let mut p: Vec<Project> =
            projects.into_iter().map(|(name, (output, sessions))| Project { name, output, sessions }).collect();
        p.sort_by(|a, b| b.output.cmp(&a.output).then(a.name.cmp(&b.name)));
        o.projects = p;
        let mut top = self.top;
        top.sort_by_key(|s| std::cmp::Reverse(s.output));
        top.truncate(8);
        o.top = top;
        o
    }
}

/// The chart's buckets for `from..to` and their length: hours for a day, days
/// for up to two months, else weeks; filled from the activity per hour.
fn buckets(mut hourly: HashMap<i64, Activity>, from: i64, to: i64, offset: Offset) -> (i64, Vec<(i64, Activity)>) {
    // `from` is a local midnight; for all time, start at the first active day.
    // Things without a time of their own (0) don't start all time in 1970.
    hourly.retain(|h, _| *h > 0);
    let first = if from > 0 {
        from
    } else {
        hourly.keys().min().map_or(day_start_at(to, offset), |&h| day_start_at(h, offset))
    };
    // One day (today, with its 23 or 25 hours on a clock change): by the hour.
    let bucket = if day_start_at(to - 1, offset) <= first {
        HOUR
    } else if to - first <= 62 * DAY {
        DAY
    } else {
        WEEK
    };
    let start = if bucket == WEEK {
        // Weeks start on Monday.
        day_start_at(first - weekday_and_hour(first, offset).0 as i64 * DAY + DAY / 2, offset)
    } else {
        first
    };
    // Days and weeks start at local midnights: a day with a clock change is
    // an hour shorter or longer, so the steps aren't all the same length.
    let step = |i: usize| match bucket {
        HOUR => start + i as i64 * HOUR,
        _ => day_start_at(start + i as i64 * bucket + DAY / 2, offset),
    };
    // A day is shown whole (to its next midnight), later hours empty.
    let end = if bucket == HOUR { day_start_at(start + DAY + DAY / 2, offset) } else { to };
    let mut n = 1;
    while n < 400 && step(n) < end {
        n += 1;
    }
    let mut buckets: Vec<(i64, Activity)> = (0..n).map(|i| (step(i), Activity::default())).collect();
    let after = step(n);
    for (h, a) in hourly {
        // The last bucket that starts at or before the hour.
        let at = buckets.partition_point(|(from, _)| *from <= h).checked_sub(1).filter(|_| h < after);
        if let Some((_, b)) = at.and_then(|i| buckets.get_mut(i)) {
            b.prompts += a.prompts;
            b.replies += a.replies;
            b.output += a.output;
            b.agent_output += a.agent_output;
            b.tool_calls += a.tool_calls;
            b.errors += a.errors;
            b.cost += a.cost;
        }
    }
    (bucket, buckets)
}

/// Every week with a prompt or a reply in it, the latest first, summed up like
/// the overview of that week (Monday 00:00 to Monday 00:00, local time).
pub fn weeks(roots: &[Source], offset: Offset) -> Vec<crate::alerts::Week> {
    let mut by: HashMap<i64, (u32, Summary)> = HashMap::new();
    for found in transcripts(roots, 0) {
        let Some(s) = found.stats() else { continue };
        let (Some(first), Some(last)) = (s.first_at, s.last_at) else { continue };
        let agents = found.agents();
        let mut start = crate::alerts::week_of(first, offset);
        while start <= last {
            let end = crate::alerts::next_week(start, offset);
            let mut sum = s.summary(start, end);
            (sum.agent_output, sum.agent_cost) = agent_work(&agents, start, end);
            if sum.prompts > 0 || sum.replies > 0 {
                let (sessions, all) = by.entry(start).or_default();
                *sessions += 1;
                all.add(&sum);
            }
            start = end;
        }
    }
    let mut weeks: Vec<crate::alerts::Week> = by
        .into_iter()
        .map(|(start, (sessions, s))| crate::alerts::Week {
            start,
            sessions,
            prompts: s.prompts,
            tokens: s.output(),
            cost: s.total_cost(),
            working: s.working_secs(),
        })
        .collect();
    weeks.sort_by_key(|w| std::cmp::Reverse(w.start));
    weeks
}

fn memory_files(roots: &[Source]) -> u32 {
    roots
        .iter()
        .flat_map(|s| std::fs::read_dir(s.config_dir.join("projects")).into_iter().flatten().flatten())
        .map(|folder| crate::setup::memory_files(&folder.path()))
        .sum()
}

/// A tool call in a list, with the session it belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct CallRow {
    pub call: Call,
    pub session: Option<(String, String)>,
}

/// Every tool call in `from..to` over all sessions, newest first.
pub fn calls(roots: &[Source], from: i64, to: i64) -> Vec<CallRow> {
    let mut out = Vec::new();
    for found in transcripts(roots, from) {
        let Some(s) = found.stats() else { continue };
        let title = s.display_title();
        out.extend(
            s.calls
                .iter()
                .filter(|c| c.at >= from && c.at < to)
                .map(|c| CallRow { call: c.clone(), session: Some((found.id.clone(), title.clone())) }),
        );
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.call.at));
    out
}

/// A file in a list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileRow {
    pub path: String,
    pub edits: u32,
    pub reads: u32,
    pub added: u64,
    pub removed: u64,
    pub last_at: i64,
    /// WSL distribution, for Linux paths.
    pub distro: Option<String>,
}

fn add_files(map: &mut HashMap<String, FileRow>, calls: &[Call], from: i64, to: i64, distro: Option<&String>) {
    // Calls that failed or were declined changed and read nothing.
    for c in calls.iter().filter(|c| c.at >= from && c.at < to && c.error.is_none()) {
        let Some(p) = &c.path else { continue };
        let f = map.entry(p.clone()).or_insert_with(|| FileRow {
            path: p.clone(),
            distro: distro.cloned(),
            ..Default::default()
        });
        if c.edits() {
            f.edits += 1;
            f.added += u64::from(c.added);
            f.removed += u64::from(c.removed);
        } else {
            f.reads += 1;
        }
        f.last_at = f.last_at.max(c.at);
    }
}

fn file_list(map: HashMap<String, FileRow>) -> Vec<FileRow> {
    let mut v: Vec<FileRow> = map.into_values().collect();
    v.sort_by(|a, b| b.edits.cmp(&a.edits).then(b.reads.cmp(&a.reads)).then(a.path.cmp(&b.path)));
    v
}

/// Files a session read or changed.
pub fn session_files(stats: &Stats, distro: Option<&String>) -> Vec<FileRow> {
    let mut map = HashMap::new();
    add_files(&mut map, &stats.calls, i64::MIN, i64::MAX, distro);
    file_list(map)
}

/// Files read or changed in `from..to` over all sessions.
pub fn files(roots: &[Source], from: i64, to: i64) -> Vec<FileRow> {
    let mut map = HashMap::new();
    for found in transcripts(roots, from) {
        if let Some(s) = found.stats() {
            add_files(&mut map, &s.calls, from, to, found.source.distro.as_ref());
        }
    }
    file_list(map)
}

/// Every session, most recent first.
pub fn sessions(roots: &[Source], archived: &HashSet<String>) -> Vec<SessionInfo> {
    let mut out: Vec<SessionInfo> = transcripts(roots, 0)
        .into_iter()
        .filter_map(|found| {
            let s = found.stats()?;
            // What Claude Code's files take on disk; kept statistics take none of it.
            let mut size = Size::default();
            if !found.kept {
                size.add(size_of(&found.path));
                size.add(size_of(&found.path.with_extension("")));
            }
            let sum = s.summary(i64::MIN, i64::MAX);
            let agents = found.agents();
            let (agent_output, agent_cost) = agent_work(&agents, i64::MIN, i64::MAX);
            Some(SessionInfo {
                title: s.display_title(),
                project: {
                    let project = s.cwd.as_deref().map(project_name).unwrap_or_else(|| "?".into());
                    // Told apart in the list of all sessions.
                    if found.id.starts_with(crate::copilot::ID_PREFIX) {
                        format!("{project}{COPILOT_TAG}")
                    } else {
                        project
                    }
                },
                distro: found.source.distro.clone(),
                first_at: s.first_at,
                last_at: s.last_at,
                prompts: sum.prompts,
                output: sum.tokens.output + agent_output,
                cost: sum.cost + agent_cost,
                errors: sum.errors,
                size,
                archived: archived.contains(&found.id),
                kept: found.kept,
                id: found.id,
            })
        })
        .filter(|s| s.prompts > 0 || s.output > 0)
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.last_at));
    out
}

/// Disk usage of one config folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Storage {
    pub label: String,
    pub dir: PathBuf,
    pub total: Size,
    /// Top-level files and folders, largest first.
    pub entries: Vec<Usage>,
    /// Transcript folders per project, largest first.
    pub projects: Vec<Usage>,
}

/// How much space Claude Code's files take in each of `roots`.
pub fn storage(roots: &[Source]) -> Vec<Storage> {
    roots
        .iter()
        .filter_map(|source| {
            let entries = std::fs::read_dir(&source.config_dir).ok()?;
            let mut s = Storage {
                label: source.distro.as_ref().map_or("Windows".to_string(), |d| format!("WSL · {d}")),
                dir: source.config_dir.clone(),
                total: Size::default(),
                entries: Vec::new(),
                projects: Vec::new(),
            };
            for e in entries.flatten() {
                let path = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                let size = if name == "projects" {
                    // Per project, so each can be listed.
                    let mut total = Size::default();
                    for p in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                        let size = size_of(&p.path());
                        total.add(size);
                        s.projects.push(Usage::new(p.file_name().to_string_lossy(), p.path(), size));
                    }
                    total
                } else {
                    size_of(&path)
                };
                s.total.add(size);
                s.entries.push(Usage::new(name, path, size));
            }
            s.entries.sort_by_key(|u| std::cmp::Reverse(u.size.bytes));
            s.projects.sort_by_key(|u| std::cmp::Reverse(u.size.bytes));
            Some(s)
        })
        .collect()
}

/// `n` bytes in a short form: `812 KB`, `1.6 GB`.
pub fn bytes(n: u64) -> String {
    let n = n as f64;
    match n {
        n if n >= 1e9 => format!("{:.1} GB", n / 1e9),
        n if n >= 1e6 => format!("{:.0} MB", n / 1e6),
        n if n >= 1e3 => format!("{:.0} KB", n / 1e3),
        n => format!("{n:.0} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(id: &str, at: &str, out: u64, ctx: u64, tools: &str) -> String {
        format!(
            r#"{{"type":"assistant","cwd":"C:\\p\\proj","timestamp":"{at}","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":1,"cache_read_input_tokens":{ctx},"cache_creation_input_tokens":0,"output_tokens":{out},"output_tokens_details":{{"thinking_tokens":5}}}},"content":[{tools}]}}}}"#
        )
    }

    fn sample() -> Stats {
        let mut s = Stats::default();
        s.line(r#"{"type":"user","timestamp":"2026-11-11T10:00:00Z","message":{"content":"fix the bug"}}"#);
        s.line(&assistant(
            "m1",
            "2026-11-11T10:00:05Z",
            100,
            1000,
            r#"{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"a.rs","old_string":"x","new_string":"y\nz"}}"#,
        ));
        s.line(&assistant(
            "m1",
            "2026-11-11T10:00:06Z",
            100,
            1000,
            r#"{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cd x && git commit -m hi | cat"}}"#,
        ));
        s.line(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":"<tool_use_error>boom\nmore</tool_use_error>"}]}}"#);
        s.line(&assistant(
            "m2",
            "2026-11-11T11:30:00Z",
            50,
            2000,
            r#"{"type":"tool_use","id":"t3","name":"Agent","input":{"subagent_type":"Explore","description":"look"}}"#,
        ));
        s.line(r#"{"type":"user","isMeta":true,"message":{"content":"meta"}}"#);
        s.line(r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#);
        s.line(r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-11-11T11:00:00Z"}"#);
        s.line(r#"{"type":"ai-title","aiTitle":"Bug fix"}"#);
        s
    }

    #[test]
    fn a_forks_transcript_starts_with_another_sessions_reply() {
        let mut s = Stats::default();
        s.line(
            r#"{"type":"fork-context-ref","agentId":"a1","parentSessionId":"abc","parentLastUuid":"u1","contextLength":869}"#,
        );
        // The reply of the session that started the fork, with the same id as there.
        let agent =
            r#"{"type":"tool_use","id":"t1","name":"Agent","input":{"subagent_type":"fork","description":"x"}}"#;
        s.line(&assistant("m1", "2026-11-11T10:00:05Z", 1691, 500, agent));
        s.line(
            r#"{"type":"user","timestamp":"2026-11-11T10:00:06Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"go"}]}}"#,
        );
        s.line(&assistant("m2", "2026-11-11T10:00:09Z", 382, 600, ""));
        assert_eq!(s.replies.iter().map(|r| r.tokens.output).collect::<Vec<_>>(), [382]);
        assert!(s.calls.is_empty());
        // Any other transcript's first reply is its own.
        let mut s = Stats::default();
        s.line(&assistant("m1", "2026-11-11T10:00:05Z", 40, 0, ""));
        assert_eq!(s.replies.len(), 1);
    }

    #[test]
    fn reads_agent_metadata() {
        let (d, k, f) =
            agent_meta(r#"{"agentType":"fork","isFork":true,"description":"Taiga UI variant","toolUseId":"t1"}"#);
        assert_eq!((d.as_deref(), k.as_deref(), f), (Some("Taiga UI variant"), Some("fork"), false));
        assert!(agent_meta(r#"{"description":"x","finished":true}"#).2);
        assert_eq!(agent_meta("garbage"), (None, None, false));
    }

    #[test]
    fn counts_a_session() {
        let s = sample();
        let sum = s.summary(i64::MIN, i64::MAX);
        assert_eq!((sum.prompts, sum.replies, sum.agents, sum.errors, sum.compactions), (1, 2, 1, 1, 1));
        assert_eq!(sum.tokens.output, 150);
        assert_eq!(sum.tokens.thinking, 10);
        assert_eq!(sum.tool_calls, 3);
        assert_eq!((sum.added, sum.removed, sum.files_changed), (2, 1, 1));
        // The commit failed, so it doesn't count.
        assert_eq!(sum.commits, 0);
        assert_eq!(s.calls[1].error.as_deref(), Some("boom ↵ more"));
        assert_eq!(s.calls[2].detail, "Explore: look");
        assert_eq!(s.title.as_deref(), Some("Bug fix"));
        assert_eq!(s.first_prompt.as_deref(), Some("fix the bug"));
        assert_eq!(s.cwd.as_deref(), Some("C:\\p\\proj"));
        assert_eq!(s.main_model().as_deref(), Some("Opus 5.5"));
        let hit = sum.tokens.cache_hit().unwrap();
        assert!((hit - 3000.0 / 3002.0).abs() < 1e-9);
        assert_eq!(s.last_at.unwrap() - s.first_at.unwrap(), 5400);
    }

    #[test]
    fn reads_a_copilot_session() {
        let line = |kind: &str, at: &str, data: &str| {
            format!(r#"{{"type":"{kind}","data":{data},"id":"e","timestamp":"2026-11-12T{at}.000Z","parentId":null}}"#)
        };
        let mut s = Stats::default();
        for l in [
            line("session.start", "10:00:00", r#"{"sessionId":"s1","context":{"cwd":"C:\\work\\shop"}}"#),
            line("hook.start", "10:00:01", r#"{"hookType":"sessionStart"}"#),
            line("user.message", "10:00:05", r#"{"content":"Fix the cart total\nand its test"}"#),
            line(
                "session.usage_record",
                "10:00:20",
                r#"{"usage":{"model":"gpt-6-luna","modelDisplayName":"GPT-6 Luna","inputTokens":31125,"outputTokens":377,"cacheReadTokens":2304,"cacheWriteTokens":0,"reasoningTokens":256,"cost":1.0}}"#,
            ),
            line(
                "tool.execution_start",
                "10:00:21",
                r#"{"toolCallId":"c1","toolName":"powershell","arguments":{"command":"git commit -m fix"}}"#,
            ),
            line(
                "tool.execution_complete",
                "10:00:25",
                r#"{"toolCallId":"c1","success":false,"error":{"message":"nothing to commit"}}"#,
            ),
            line(
                "tool.execution_start",
                "10:00:26",
                r#"{"toolCallId":"c2","toolName":"edit","arguments":{"path":"C:\\work\\shop\\cart.rs","old_str":"a","new_str":"b\nc"}}"#,
            ),
            line("tool.execution_complete", "10:00:27", r#"{"toolCallId":"c2","success":true}"#),
            // Written again, cut off, unknown: no harm.
            line("tool.execution_start", "10:00:26", r#"{"toolCallId":"c2","toolName":"edit","arguments":{}}"#),
            r#"{"type":"session.usage_record","data":{"usage":"#.to_string(),
            line("session.usage_record", "10:00:30", r#"{"usage":"soon"}"#),
        ] {
            s.copilot_line(&l);
        }
        let sum = s.summary(i64::MIN, i64::MAX);
        assert_eq!((sum.prompts, sum.replies, sum.tool_calls, sum.errors), (1, 1, 2, 1));
        assert_eq!((sum.tokens.output, sum.tokens.cache_read, sum.tokens.input), (377, 2304, 31125 - 2304));
        assert_eq!((sum.added, sum.removed, sum.files_changed), (2, 1, 1));
        // The commit failed, so it doesn't count.
        assert_eq!(sum.commits, 0);
        assert_eq!(s.cwd.as_deref(), Some(r"C:\work\shop"));
        assert_eq!(s.first_prompt.as_deref(), Some("Fix the cart total ↵ and its test"));
        assert_eq!(s.calls[0].error.as_deref(), Some("nothing to commit"));
        assert_eq!(s.main_model().as_deref(), Some("GPT-6 Luna"));
        assert_eq!(s.last_at.unwrap() - s.first_at.unwrap(), 27);
    }

    #[test]
    fn days_start_at_local_midnight_across_a_clock_change() {
        let at = |t: &str| crate::timefmt::parse_rfc3339(t).unwrap();
        // Central Europe, autumn: at 01:00 UTC on 25 October the clocks go from 03:00 back to 02:00.
        let switch = at("2026-10-25T01:00:00Z");
        let offset = move |t: i64| if t < switch { 7200 } else { 3600 };
        // The 25th starts at 22:00 UTC the day before, seen from before and after the change.
        assert_eq!(day_start_at(at("2026-10-24T22:30:00Z"), &offset), at("2026-10-24T22:00:00Z"));
        assert_eq!(day_start_at(at("2026-10-25T11:00:00Z"), &offset), at("2026-10-24T22:00:00Z"));
        // It has 25 hours: the 26th starts at 23:00 UTC.
        assert_eq!(day_start_at(at("2026-10-25T23:30:00Z"), &offset), at("2026-10-25T23:00:00Z"));
        assert_eq!(day_start_at(at("2026-10-25T22:30:00Z"), &offset), at("2026-10-24T22:00:00Z"));
        // A week back from the 27th is the 21st, at its own midnight (22:00 UTC, not 23:00).
        let today = day_start_at(at("2026-10-27T10:00:00Z"), &offset);
        assert_eq!(today, at("2026-10-26T23:00:00Z"));
        assert_eq!(Period::Week.start(today, &offset), at("2026-10-20T22:00:00Z"));
        assert_eq!(Period::Today.start(today, &offset), today);
    }

    #[test]
    fn today_is_compared_with_yesterday_until_the_same_time() {
        let at = |t: &str| crate::timefmt::parse_rfc3339(t).unwrap();
        let utc = |_: i64| 0;
        let (today, now) = (at("2026-11-19T00:00:00Z"), at("2026-11-19T10:00:00Z"));
        // Yesterday's morning, not its last ten hours.
        assert_eq!(
            previous_range(Period::Today, today, now, &utc),
            Some((at("2026-11-18T00:00:00Z"), at("2026-11-18T10:00:00Z")))
        );
        // The other periods: as long a time right before.
        let week = Period::Week.start(today, &utc);
        assert_eq!(previous_range(Period::Week, week, now, &utc), Some((week - (now - week), week)));
        assert_eq!(previous_range(Period::All, 0, now, &utc), None);
        // The day after the clocks went back: yesterday had 25 hours, and never reaches into today.
        let switch = at("2026-10-25T01:00:00Z");
        let offset = move |t: i64| if t < switch { 7200 } else { 3600 };
        let today = day_start_at(at("2026-10-26T10:00:00Z"), &offset);
        let before = |len: i64| previous_range(Period::Today, today, today + len, &offset).unwrap();
        assert_eq!(before(10 * HOUR), (at("2026-10-24T22:00:00Z"), at("2026-10-25T08:00:00Z")));
        assert_eq!(before(26 * HOUR), (at("2026-10-24T22:00:00Z"), today));
    }

    #[test]
    fn counts_lines_written_again_once() {
        let mut s = sample();
        let before = s.summary(i64::MIN, i64::MAX);
        let prompt =
            r#"{"type":"user","uuid":"u1","timestamp":"2026-11-11T12:00:00Z","message":{"content":"and the test"}}"#;
        s.line(prompt);
        // Claude Code wrote these again: the first reply with its tool call, and the prompt.
        s.line(&assistant(
            "m1",
            "2026-11-11T10:00:05Z",
            100,
            1000,
            r#"{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"a.rs","old_string":"x","new_string":"y\nz"}}"#,
        ));
        s.line(prompt);
        let after = s.summary(i64::MIN, i64::MAX);
        assert_eq!(after.prompts, before.prompts + 1);
        assert_eq!((after.replies, after.tool_calls), (before.replies, before.tool_calls));
        assert_eq!(after.tokens.output, before.tokens.output);
    }

    #[test]
    fn sums_a_window() {
        let s = sample();
        let at = |t: &str| crate::timefmt::parse_rfc3339(t).unwrap();
        let sum = s.summary(at("2026-11-11T11:00:00Z"), at("2026-11-11T12:00:00Z"));
        assert_eq!((sum.prompts, sum.replies, sum.tool_calls, sum.compactions), (0, 1, 1, 1));
        assert_eq!(sum.tokens.output, 50);
    }

    #[test]
    fn prices_replies() {
        let m = 1_000_000;
        let t = Tokens { input: m, output: m, cache_read: m, cache_write: m, cache_write_1h: m / 2, thinking: 0 };
        // 4 input + 20 output + 0.20 read + 0.5M at 5 (5 min) + 0.5M at 8 (1 h).
        assert!((cost("claude-opus-5-5", &t) - (4.0 + 20.0 + 0.2 + 2.5 + 4.0)).abs() < 1e-9);
        assert!((cost("claude-opus-5", &Tokens { output: m, ..Default::default() }) - 25.0).abs() < 1e-9);
        assert!((cost("claude-haiku-4-5-20251001", &Tokens { input: m, ..Default::default() }) - 1.0).abs() < 1e-9);
        assert_eq!(cost("<synthetic>", &t), 0.0);
        // Claude 3's ids have the version first.
        let output = Tokens { output: m, ..Default::default() };
        for (id, dollars) in [
            ("claude-3-7-sonnet-20250219", 15.0),
            ("claude-3-5-sonnet-20241022", 15.0),
            ("claude-3-5-haiku-20241022", 4.0),
            ("claude-3-haiku-20240307", 1.25),
            ("claude-3-opus-20240229", 75.0),
        ] {
            assert!((cost(id, &output) - dollars).abs() < 1e-9, "{id}");
        }
        assert_eq!(family_first("opus-5-5"), "opus-5-5");
        assert_eq!(family_first("3-5"), "3-5");
        assert_eq!(dollars(0.4249), "$0.42");
        assert_eq!(dollars(1234.0), "$1.2k");
    }

    #[test]
    fn groups_failures() {
        let call = |detail: &str, error: &str| Call {
            tool: "Bash".into(),
            detail: detail.into(),
            error: Some(error.into()),
            ..Default::default()
        };
        let a = call("cd x && cargo build", r"Exit code 101 ↵ error[E0308]: mismatched types in C:\x\a.rs:12");
        let b = call("cargo build --release", r"Exit code 101 ↵ error[E0277]: mismatched types in C:\y\b.rs:40");
        assert_eq!(a.signature().as_deref(), Some("cargo: error[E#]: mismatched types in …"));
        assert_eq!(a.signature(), b.signature());
        let declined = call("rm -rf x", "The user doesn't want to proceed with this tool use.");
        assert_eq!(declined.signature(), None);
        let groups = failures([a, b, declined].iter());
        assert_eq!((groups.len(), groups[0].count), (1, 2));
    }

    #[test]
    fn ends_turns_at_long_pauses() {
        let mut s = Stats::default();
        s.line(r#"{"type":"user","timestamp":"2026-11-11T10:00:00Z","message":{"content":"go"}}"#);
        s.line(&assistant("a", "2026-11-11T10:10:00Z", 1, 1, ""));
        // Hours later a background task reports back and Claude answers it.
        s.line(&assistant("b", "2026-11-11T15:00:00Z", 1, 1, ""));
        s.line(r#"{"type":"user","isCompactSummary":true,"timestamp":"2026-11-11T16:00:00Z","message":{"content":"This session is being continued"}}"#);
        let turns = s.turns();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].secs, 600);
    }

    #[test]
    fn measures_turns() {
        let s = sample();
        let turns = s.turns();
        assert_eq!(turns.len(), 1);
        assert_eq!((turns[0].secs, turns[0].prompt.as_str()), (6, "fix the bug"));
        assert_eq!(s.summary(i64::MIN, i64::MAX).median_turn(), Some(6));
    }

    #[test]
    fn finds_programs() {
        assert_eq!(programs("cd x && git status && cargo build | head"), ["git", "cargo"]);
        assert_eq!(programs("FOO=1 C:\\bin\\rg.exe -n x"), ["rg"]);
        assert_eq!(programs(r"Set-Location C:\x; npm test"), ["npm"]);
        assert_eq!(programs("cd x && python - <<'EOF' ↵ import sys ↵ print(1) ↵ EOF"), ["python"]);
        assert!(programs("").is_empty());
    }

    #[test]
    fn sums_sessions_per_project() {
        let s = |project: &str, prompts: u32, last: i64| SessionInfo {
            id: format!("{project}{last}"),
            title: String::new(),
            project: project.into(),
            distro: None,
            first_at: Some(last - 100),
            last_at: Some(last),
            prompts,
            output: 1000,
            errors: 1,
            cost: 0.5,
            size: Size::default(),
            archived: false,
            kept: false,
        };
        let p = projects(&[s("shop", 3, 200), s("api", 1, 900), s("shop · Copilot", 2, 500)]);
        assert_eq!(p.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["api", "shop"]);
        let shop = &p[1];
        assert_eq!((shop.sessions, shop.prompts, shop.output, shop.errors), (2, 5, 2000, 2));
        assert_eq!((shop.first_at, shop.last_at), (Some(100), Some(500)));
        assert!((shop.cost - 1.0).abs() < 1e-9);
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(812_000), "812 KB");
        assert_eq!(bytes(1_600_000_000), "1.6 GB");
    }

    /// Kept statistics bring along which calls are still open, by their place in
    /// the list. A file that doesn't fit itself (damaged, or changed by hand) must
    /// not end the app: an error for a call that isn't there counts on its own.
    #[test]
    fn an_error_for_a_call_that_is_not_there_counts_alone() {
        let mut s: Stats = serde_json::from_str(r#"{"open":{"t9":5,"c9":7}}"#).unwrap();
        s.line(
            r#"{"type":"user","timestamp":"2026-11-11T10:00:00Z","message":{"content":[{"type":"tool_result","tool_use_id":"t9","is_error":true,"content":"boom"}]}}"#,
        );
        assert_eq!((s.calls.len(), s.orphan_errors), (0, 1));
        s.copilot_line(
            r#"{"type":"tool.execution_complete","timestamp":"2026-11-11T10:00:01Z","data":{"toolCallId":"c9","success":false,"error":{"message":"boom"}}}"#,
        );
        assert_eq!((s.calls.len(), s.orphan_errors), (0, 2));
        // A model id of Claude 3, which has its version first, and ones that only look like it.
        assert_eq!(family_first("3-7-sonnet-20250219"), "sonnet-3-7-20250219");
        assert_eq!(family_first("3-7"), "3-7");
        assert_eq!(family_first("3"), "3");
    }

    #[test]
    fn overview_of_files() {
        let dir = std::env::temp_dir().join(format!("trayward-stats-{}", std::process::id()));
        let folder = dir.join("projects").join("C--p-proj");
        std::fs::create_dir_all(folder.join("abc").join("subagents")).unwrap();
        let now = crate::timefmt::parse_rfc3339("2026-11-11T12:00:00Z").unwrap();
        std::fs::write(
            folder.join("abc.jsonl"),
            format!(
                "{}\n{}\n",
                r#"{"type":"user","timestamp":"2026-11-11T10:00:00Z","cwd":"C:\\p\\proj","message":{"content":"hi"}}"#,
                assistant(
                    "m1",
                    "2026-11-11T10:00:05Z",
                    100,
                    10,
                    r#"{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"b.rs"}}"#
                )
            ),
        )
        .unwrap();
        std::fs::write(
            folder.join("abc").join("subagents").join("agent-1.jsonl"),
            format!("{}\n", assistant("s1", "2026-11-11T10:01:00Z", 40, 0, "")),
        )
        .unwrap();
        // An older copy of the same session in the folder of its project's old place.
        let old = dir.join("projects").join("C--old-proj");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::copy(folder.join("abc.jsonl"), old.join("abc.jsonl")).unwrap();
        let file = std::fs::File::options().write(true).open(old.join("abc.jsonl")).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000)).unwrap();
        drop(file);
        let roots = [Source { distro: None, config_dir: dir.clone() }];
        let today = day_start_at(now, &|_| 0);
        let o = overview(&roots, Period::Week.start(today, &|_| 0), now, &|_| 0, &HashSet::new());
        assert_eq!((o.sessions, o.summary.prompts, o.summary.replies), (1, 1, 1));
        assert_eq!(o.summary.agent_output, 40);
        assert_eq!(o.bucket, DAY);
        assert_eq!(o.buckets.len(), 7);
        assert_eq!(o.buckets[6].1.output, 100);
        assert_eq!(o.buckets[6].1.agent_output, 40);
        assert_eq!(o.projects[0].name, "proj");
        assert_eq!(o.projects[0].output, 140);
        assert_eq!(o.summary.files_read, 1);
        // 2026-11-11 was a Wednesday.
        assert_eq!(o.heat[2][10], 2);
        // Its model: what it wrote, and the subagent's replies counted too.
        assert!(!o.models.is_empty() && o.models.iter().map(|m| m.output).sum::<u64>() >= 100);
        // One project's overview: this one, or none of another.
        let week = Period::Week.start(today, &|_| 0);
        assert_eq!(overview_of(&roots, week, now, &|_| 0, &HashSet::new(), Some("proj")).sessions, 1);
        assert_eq!(overview_of(&roots, week, now, &|_| 0, &HashSet::new(), Some("other")).sessions, 0);
        let today_view = overview(&roots, Period::Today.start(today, &|_| 0), now, &|_| 0, &HashSet::new());
        assert_eq!(today_view.bucket, HOUR);
        assert_eq!(today_view.buckets[10].1.prompts, 1);
        let list = sessions(&roots, &HashSet::from(["abc".to_string()]));
        assert_eq!(list.len(), 1);
        assert!(list[0].archived);
        assert_eq!(list[0].title, "hi");
        assert_eq!(files(&roots, 0, now)[0].path, "b.rs");
        assert_eq!(calls(&roots, 0, now).len(), 1);
        // Its week, from Monday 2026-11-09: the same sums as the overview's.
        let all = weeks(&roots, &|_| 0);
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].start, all[0].sessions, all[0].prompts, all[0].tokens), (1_794_182_400, 1, 1, 140));
        assert!((all[0].cost - o.summary.total_cost()).abs() < 1e-9);
        let data = session(&roots, "abc").unwrap();
        assert_eq!(data.summary.agent_output, 40);
        assert!(data.storage.iter().any(|u| u.label == "Subagent transcripts" && u.size.files == 1));
        // Its model, with the subagent's reply counted in.
        let models: Vec<_> = data.models.iter().map(|m| (m.name.as_str(), m.output, m.replies)).collect();
        assert_eq!(models, [("Opus 5.5", 140, 2)]);
        assert!(storage(&roots)[0].total.files >= 2);

        // Kept statistics: saved, then the transcripts are deleted, and the session still counts.
        let kept = dir.join("kept");
        crate::keep::use_dir(&kept);
        crate::keep::set_enabled(true);
        assert_eq!(crate::keep::keep_all(&roots, crate::os::now_ms() / 1000 + 3600), 1);
        assert_eq!(crate::keep::keep_all(&roots, crate::os::now_ms() / 1000 + 3600), 0, "unchanged: not saved again");
        std::fs::remove_dir_all(dir.join("projects")).unwrap();
        forget();
        let o = overview(&roots, Period::Week.start(today, &|_| 0), now, &|_| 0, &HashSet::new());
        assert_eq!((o.sessions, o.summary.prompts, o.summary.agent_output), (1, 1, 40));
        let list = sessions(&roots, &HashSet::new());
        assert!(list[0].kept && list[0].title == "hi" && list[0].size.bytes == 0);
        assert_eq!(files(&roots, 0, now)[0].path, "b.rs");
        let data = session(&roots, "abc").unwrap();
        assert!(data.kept);
        assert_eq!(data.summary.agent_output, 40);
        assert_eq!((data.models.len(), data.models[0].output), (1, 140));
        // Switched off: not counted, and deleted.
        crate::keep::set_enabled(false);
        assert_eq!(overview(&roots, Period::Week.start(today, &|_| 0), now, &|_| 0, &HashSet::new()).sessions, 0);
        crate::keep::delete_all();
        assert!(!kept.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The statistics start from what was kept of a session and read only what
    /// its transcripts got since; what doesn't fit is read from the start.
    #[test]
    fn reads_on_from_what_was_kept() {
        use std::io::{Seek, SeekFrom, Write};
        let dir = std::env::temp_dir().join(format!("trayward-stats-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("projects").join("C--p-proj");
        std::fs::create_dir_all(folder.join("abc").join("subagents")).unwrap();
        crate::keep::use_dir(&dir.join("kept"));
        crate::keep::set_enabled(true);
        let transcript = folder.join("abc.jsonl");
        let agent = folder.join("abc").join("subagents").join("agent-1.jsonl");
        // Written at times of the test's choosing (seconds after `T`), long before `later`.
        const T: u64 = 1_800_000_000;
        let later = (T + 100_000) as i64;
        let at = |path: &Path, secs: u64| {
            let file = std::fs::File::options().write(true).open(path).unwrap();
            file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(T + secs)).unwrap();
        };
        let written = |path: &Path| modified(&std::fs::metadata(path).unwrap()) - T as i64;
        // Writes `text` over what's at the last place `find` is at.
        let overwrite = |find: &str, text: &str| {
            let place = std::fs::read_to_string(&transcript).unwrap().rfind(find).unwrap();
            let mut file = std::fs::File::options().write(true).open(&transcript).unwrap();
            file.seek(SeekFrom::Start(place as u64)).unwrap();
            file.write_all(text.as_bytes()).unwrap();
        };
        let prompt = r#"{"type":"user","uuid":"u1","timestamp":"2026-11-11T10:00:00Z","cwd":"C:\\p\\proj","message":{"content":"first"}}"#;
        // Long enough that the prompt isn't among the bytes checked before reading on.
        let text = format!(r#"{{"type":"text","text":"{}"}}"#, "x".repeat(400));
        let reply = assistant("m1", "2026-11-11T10:00:05Z", 100, 10, &text);
        std::fs::write(&transcript, format!("{prompt}\n{reply}\n")).unwrap();
        std::fs::write(&agent, format!("{}\n", assistant("s1", "2026-11-11T10:01:00Z", 40, 0, ""))).unwrap();
        at(&transcript, 0);
        at(&agent, 0);
        let roots = [Source { distro: None, config_dir: dir.clone() }];
        let all = || overview(&roots, 0, i64::MAX / 2, &|_| 0, &HashSet::new());
        let title = || sessions(&roots, &HashSet::new())[0].title.clone();
        let o = all();
        assert_eq!((o.sessions, o.summary.prompts, o.summary.replies, o.summary.agent_output), (1, 1, 1, 40));

        // Read for the first time: kept right away, with where reading stopped. Not as "all
        // until then" (its time is a second earlier): more may be written in the same second.
        let kept = dir.join("kept").join("windows").join("abc.json");
        let offset = || {
            let text = std::fs::read_to_string(&kept).unwrap();
            assert!(
                text.starts_with(&format!(r#"{{"version":2,"parser":{PARSER},"#)),
                "{}",
                text.get(..40).unwrap_or(&text)
            );
            let (_, rest) = text.split_once(r#""offset":"#).unwrap();
            rest.split(',').next().unwrap().parse::<u64>().unwrap()
        };
        let len = || std::fs::metadata(&transcript).unwrap().len();
        assert_eq!((offset(), written(&kept)), (len(), -1));

        // A later run: the prompt's text changed meanwhile (which no transcript does), the
        // first reply was written again (as Claude Code does) and a new one was added. Only
        // what was added is read: the text is the one that was kept, the reply counts once.
        forget();
        overwrite("first", "FIRST");
        let mut file = std::fs::File::options().append(true).open(&transcript).unwrap();
        writeln!(file, "{reply}\n{}", assistant("m2", "2026-11-11T10:02:00Z", 7, 10, "")).unwrap();
        drop(file);
        at(&transcript, 10);
        let o = all();
        assert_eq!((o.summary.prompts, o.summary.replies, o.summary.tokens.output), (1, 2, 107));
        assert_eq!((o.summary.agent_output, title().as_str()), (40, "first"));
        // Too little is new to write the kept file again: `keep` does, and then nothing is new.
        assert!(offset() < len());
        assert_eq!(crate::keep::keep_all(&roots, later), 1);
        assert_eq!(crate::keep::keep_all(&roots, later), 0);
        assert_eq!((offset(), written(&kept)), (len(), 10));

        // A subagent's transcript that grew is read again; the session's isn't.
        forget();
        let mut file = std::fs::File::options().append(true).open(&agent).unwrap();
        writeln!(file, "{}", assistant("s2", "2026-11-11T10:03:00Z", 2, 0, "")).unwrap();
        drop(file);
        at(&agent, 20);
        let o = all();
        assert_eq!((o.summary.replies, o.summary.agent_output, title().as_str()), (2, 42, "first"));
        assert_eq!(crate::keep::keep_all(&roots, later), 1);
        assert_eq!(written(&kept), 20);

        // The bytes before where reading stopped aren't the ones from then: another file,
        // read from its start.
        forget();
        overwrite("10:02:00Z", "10:02:01Z");
        at(&transcript, 30);
        assert_eq!(title(), "FIRST");
        assert_eq!(crate::keep::keep_all(&roots, later), 1);
        assert_eq!((offset(), written(&kept)), (len(), 30));

        // Kept by a version that counted differently: `keep` makes it again from the
        // transcript, although the session wasn't written since.
        forget();
        overwrite("FIRST", "first");
        at(&transcript, 30);
        assert_eq!(title(), "FIRST");
        forget();
        let text = std::fs::read_to_string(&kept).unwrap();
        std::fs::write(&kept, text.replace(&format!(r#""parser":{PARSER},"#), r#""parser":0,"#)).unwrap();
        at(&kept, 30);
        assert_eq!(crate::keep::keep_all(&roots, later), 1);
        assert_eq!(crate::keep::keep_all(&roots, later), 0);
        forget();
        assert_eq!(title(), "first");

        crate::keep::set_enabled(false);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A distribution that isn't running counts from what was kept of it, once,
    /// however many users' folders it has when it runs.
    #[test]
    fn counts_a_stopped_distribution_from_what_was_kept() {
        let dir = std::env::temp_dir().join(format!("trayward-stats-stopped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("ubuntu").join("projects").join("-home-me-proj");
        std::fs::create_dir_all(&folder).unwrap();
        crate::keep::use_dir(&dir.join("kept"));
        crate::keep::set_enabled(true);
        std::fs::write(
            folder.join("abc.jsonl"),
            format!(
                "{}\n{}\n",
                r#"{"type":"user","timestamp":"2026-11-11T10:00:00Z","cwd":"/home/me/proj","message":{"content":"hi"}}"#,
                assistant("m1", "2026-11-11T10:00:05Z", 100, 10, "")
            ),
        )
        .unwrap();
        let ubuntu = |config: &str| Source { distro: Some("Ubuntu".into()), config_dir: dir.join(config) };
        let windows = Source { distro: None, config_dir: dir.join("windows") };
        // Running, with a second user's folder that has nothing: read, and kept.
        let running = [windows.clone(), ubuntu("root"), ubuntu("ubuntu")];
        let all = |roots: &[Source]| overview(roots, 0, i64::MAX / 2, &|_| 0, &HashSet::new());
        assert_eq!(all(&running).sessions, 1);
        assert!(dir.join("kept").join("wsl-Ubuntu").join("abc.json").is_file());
        assert!(crate::keep::stopped(&running).is_empty());
        // Stopped: it's not among the places, but what was kept of it is.
        forget();
        let mut roots = vec![windows];
        assert_eq!(all(&roots).sessions, 0);
        roots.extend(crate::keep::stopped(&roots));
        assert_eq!(roots[1].distro.as_deref(), Some("Ubuntu"));
        let o = all(&roots);
        assert_eq!((o.sessions, o.summary.prompts, o.summary.tokens.output), (1, 1, 100));
        let list = sessions(&roots, &HashSet::new());
        assert!(list[0].kept && list[0].distro.as_deref() == Some("Ubuntu"));
        assert!(session(&roots, "abc").unwrap().kept);
        assert!(storage(&roots).is_empty());
        // Running again, its transcript counts, and what's kept isn't counted on top.
        forget();
        assert_eq!(all(&running).sessions, 1);
        assert!(!sessions(&running, &HashSet::new())[0].kept);

        crate::keep::set_enabled(false);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod timing {
    /// `cargo test --release timing -- --ignored --nocapture`: reads the real transcripts,
    /// first all of them, then again starting from kept statistics (kept in a folder of
    /// the test's own, not Trayward's), which must give the same numbers.
    #[test]
    #[ignore]
    fn all_time_on_real_data() {
        let dir = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap()).join(".claude");
        let roots = [crate::registry::Source { distro: None, config_dir: dir }];
        let now = crate::os::now_ms() / 1000;
        let kept = std::env::temp_dir().join(format!("trayward-timing-kept-{}", std::process::id()));
        crate::keep::use_dir(&kept);
        let numbers = |o: &super::Overview| {
            let s = &o.summary;
            (o.sessions, s.prompts, s.replies, s.tool_calls, s.errors, s.tokens.output, s.agent_output, s.files_changed)
        };
        let read = |round: &str| {
            let t = std::time::Instant::now();
            let o = super::overview(&roots, 0, now, &|_| 0, &Default::default());
            println!("{round}: {:?} in {:?}", numbers(&o), t.elapsed());
            o
        };
        let cold = read("everything, nothing kept");
        read("again");
        super::forget();
        crate::keep::set_enabled(true);
        let keeping = read("everything, keeping it");
        super::forget();
        let resumed = read("from what was kept");
        // Sessions written to in between have more in them: told apart by the time they end.
        assert_eq!(numbers(&cold), numbers(&keeping), "a session was written to meanwhile? Run it again.");
        assert_eq!(numbers(&keeping), numbers(&resumed), "a session was written to meanwhile? Run it again.");
        let size: u64 = super::walk(&kept).bytes;
        println!("kept: {}", super::bytes(size));
        let t = std::time::Instant::now();
        let s = super::storage(&roots);
        println!("storage: {} in {:?}", super::bytes(s[0].total.bytes), t.elapsed());
        crate::keep::set_enabled(false);
        let _ = std::fs::remove_dir_all(&kept);
    }
}
