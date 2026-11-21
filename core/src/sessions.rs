//! Claude Code sessions and how they move between states.
//!
//! Two sources feed this, and both are optional:
//! - Claude Code's own list of running sessions (`~/.claude/sessions/*.json`,
//!   read by `registry.rs`). It covers every open session, hooks or not, with
//!   a coarse state: busy, waiting for the user, or idle.
//! - Hook events (`hook.rs`). They arrive instantly, count subagents, and tell
//!   a finished turn (`Stop`) apart from an interrupted one.
//!
//! Each source only overrides the state when its information is newer than the
//! last change. Pure logic, no Win32, so it is unit-tested.

use crate::hook::HookMessage;
use crate::transcripts::{Stats, WatchKind, Watcher};
use std::collections::{BTreeMap, HashSet};

/// Background tasks older than this are assumed gone.
const WATCH_MAX_SECS: i64 = 24 * 3600;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Claude is working on a prompt.
    Working,
    /// Claude is waiting for you: a permission prompt or a question.
    NeedsInput,
    /// Claude finished and you haven't looked at it yet.
    Done,
    /// Nothing happening (seen, interrupted, or just opened).
    Idle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub kind: String,
    /// When the hook said it started (unix ms).
    #[serde(default)]
    pub started_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    /// Title from the hook input, when Claude Code sent one.
    pub title: Option<String>,
    /// Title, model and token counts from the transcript.
    #[serde(default)]
    pub stats: Stats,
    /// Name of the VS Code window the session belongs to, e.g. "🚀 Shop".
    #[serde(default)]
    pub workspace: Option<String>,
    /// WSL distribution the session runs in, or `None` for Windows.
    #[serde(default)]
    pub distro: Option<String>,
    /// Claude Code's short session name, e.g. `web-game-01`.
    #[serde(default)]
    pub name: Option<String>,
    /// Start of the most recent prompt.
    pub last_prompt: Option<String>,
    pub status: Status,
    /// When `status` last changed (unix ms).
    pub since_ms: i64,
    /// Last event from any source (unix ms).
    pub updated_ms: i64,
    /// Subagents currently running.
    pub agents: Vec<Agent>,
    pub claude_pid: Option<u32>,
    pub claude_started: Option<u64>,
    /// When the Claude Code process started (unix ms); older background tasks died with an earlier one.
    #[serde(default)]
    pub process_started_ms: Option<i64>,
    /// VS Code's main process, when the session runs in VS Code.
    pub ide_pid: Option<u32>,
    /// How the session was started, e.g. `claude-vscode` or `cli`.
    pub entrypoint: Option<String>,
    /// Hook events arrive for this session.
    #[serde(default)]
    pub hooked: bool,
    /// When the session last finished a turn (unix ms); kept after it was seen.
    #[serde(default)]
    pub done_at: Option<i64>,
    /// Archived in the Claude Code extension.
    #[serde(default)]
    pub archived: bool,
    /// A tool event took "needs you" away: when, and since when it had needed
    /// the user. The event may have come from other work running in parallel
    /// (a subagent, another tool call) while the prompt is still open; the
    /// session list then says so (see `sync`).
    #[serde(skip)]
    tool_cleared: Option<(i64, i64)>,
    /// "Needs you" was given back after a tool event took it away: the
    /// prompt was announced already, and isn't again.
    #[serde(skip)]
    pub restored: bool,
}

impl Session {
    fn new(id: &str, status: Status, at: i64) -> Session {
        Session {
            id: id.to_string(),
            cwd: String::new(),
            title: None,
            stats: Stats::default(),
            workspace: None,
            distro: None,
            name: None,
            last_prompt: None,
            status,
            since_ms: at,
            updated_ms: at,
            agents: Vec::new(),
            claude_pid: None,
            claude_started: None,
            process_started_ms: None,
            ide_pid: None,
            entrypoint: None,
            hooked: false,
            done_at: (status == Status::Done).then_some(at),
            archived: false,
            tool_cleared: None,
            restored: false,
        }
    }

    fn set_status(&mut self, status: Status, at: i64) {
        if self.status != status {
            self.tool_cleared = None;
            self.restored = false;
            self.status = status;
            self.since_ms = at;
            match status {
                Status::Done => self.done_at = Some(at),
                Status::Working | Status::NeedsInput => self.done_at = None,
                Status::Idle => {}
            }
        }
    }

    /// What the session is called in lists: its VS Code window's name, else its folder's.
    pub fn display_name(&self) -> String {
        self.workspace.clone().unwrap_or_else(|| self.project().to_string())
    }

    /// Folder name of the working directory, e.g. `my-app`.
    pub fn project(&self) -> &str {
        let trimmed = self.cwd.trim_end_matches(['\\', '/']);
        trimmed.rsplit(['\\', '/']).next().filter(|s| !s.is_empty()).unwrap_or("Claude")
    }

    /// Finished within `window_ms` before `at`, seen or not.
    pub fn recently_done(&self, at: i64, window_ms: i64) -> bool {
        matches!(self.status, Status::Done | Status::Idle) && self.done_at.is_some_and(|d| at - d < window_ms)
    }

    /// The best available description of the conversation.
    pub fn description(&self) -> Option<&str> {
        [&self.stats.title, &self.title, &self.last_prompt, &self.name]
            .into_iter()
            .find_map(|t| t.as_deref().filter(|t| !t.trim().is_empty()))
    }

    pub fn in_vscode(&self) -> bool {
        self.entrypoint.as_deref() == Some("claude-vscode")
    }

    /// Background work the session left running (commands, monitors,
    /// background agents, scheduled wake-ups) that is still going at `at_ms`.
    pub fn watchers(&self, at_ms: i64) -> Vec<&Watcher> {
        let t = at_ms / 1000;
        // Tasks from before the current process started died with the old one.
        let after = self.process_started_ms.map_or(i64::MIN, |p| p / 1000 - 5);
        self.stats
            .watchers
            .iter()
            .filter(|w| w.since >= after && w.until.is_none_or(|u| u > t) && t - w.since < WATCH_MAX_SECS)
            .collect()
    }

    /// Subagents running at `at_ms`: counted by hooks, or seen in the
    /// transcript (agents the turn waits for, and background agents, which
    /// keep running after the turn is done).
    pub fn running_agents(&self, at_ms: i64) -> usize {
        let background = self.watchers(at_ms).iter().filter(|w| w.kind == WatchKind::Agent).count();
        // Like the watchers: a call from before this process started died with the old one.
        let (t, after) = (at_ms / 1000, self.process_started_ms.map_or(i64::MIN, |p| p / 1000 - 5));
        let waited_for = self.stats.agent_calls.iter().filter(|c| **c >= after && t - **c < WATCH_MAX_SECS).count();
        self.agents.len().max(waited_for + background)
    }
}

/// State of a session in Claude Code's own session list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListedState {
    Busy,
    Waiting,
    Idle,
    Unknown,
}

impl ListedState {
    pub fn parse(s: &str) -> ListedState {
        match s {
            "busy" => ListedState::Busy,
            "waiting" => ListedState::Waiting,
            "idle" => ListedState::Idle,
            _ => ListedState::Unknown,
        }
    }
}

/// A running session as Claude Code lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub id: String,
    pub pid: u32,
    pub started: Option<u64>,
    pub cwd: String,
    pub name: Option<String>,
    pub state: ListedState,
    /// When `state` last changed (unix ms).
    pub state_at_ms: i64,
    pub entrypoint: Option<String>,
    /// WSL distribution, or `None` for Windows.
    pub distro: Option<String>,
    /// When the Claude Code process started (unix ms).
    pub started_ms: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sessions {
    list: Vec<Session>,
    /// Sessions the user hid, and when (unix ms). Kept when a session's process
    /// ends, so it stays hidden when it is resumed.
    #[serde(default)]
    hidden: BTreeMap<String, i64>,
    /// Sessions archived in the Claude Code extension, as last read; new ones start so.
    #[serde(skip)]
    archived: HashSet<String>,
}

/// Hidden sessions that are gone are forgotten after this long.
const HIDDEN_KEEP_MS: i64 = 30 * 24 * 3600 * 1000;

/// Notification types that mean Claude is blocked on the user.
const NEEDS_INPUT: &[&str] = &["permission_prompt", "elicitation_dialog", "agent_needs_input"];

/// How long a hooked session that went idle waits for its `Stop` hook before
/// it counts as interrupted rather than done.
const STOP_GRACE_MS: i64 = 4_000;

impl Sessions {
    pub fn get(&self, id: &str) -> Option<&Session> {
        self.list.iter().find(|s| s.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Session> {
        self.list.iter()
    }

    fn index_or_insert(&mut self, id: &str, status: Status, at: i64) -> usize {
        match self.list.iter().position(|s| s.id == id) {
            Some(i) => i,
            None => {
                let mut s = Session::new(id, status, at);
                s.archived = self.archived.contains(id);
                self.list.push(s);
                self.list.len() - 1
            }
        }
    }

    /// The list without when each session was last heard of: every event
    /// changes that, and nothing shown depends on it.
    fn shown(&self) -> (Vec<Session>, usize) {
        let list = self.list.iter().map(|s| Session { updated_ms: 0, ..s.clone() }).collect();
        (list, self.hidden.len())
    }

    /// Applies one hook event. Returns whether anything changed that shows
    /// (when the session was last heard of doesn't count).
    pub fn apply(&mut self, m: &HookMessage) -> bool {
        let before = self.shown();
        self.apply_inner(m);
        // A prompt of the user's own shows a hidden session again; a message
        // from another session or a task notification doesn't.
        if m.event == "UserPromptSubmit" && m.prompt.as_deref().is_some_and(crate::transcripts::is_user_text) {
            self.hidden.remove(&m.session_id);
        }
        self.unhide_needing();
        self.shown() != before
    }

    fn apply_inner(&mut self, m: &HookMessage) {
        if m.session_id.is_empty() {
            return;
        }
        if m.event == "SessionEnd" {
            self.list.retain(|s| s.id != m.session_id);
            return;
        }
        let at = m.at_ms;
        let needs_input = m.notification_type.as_deref().is_some_and(|t| NEEDS_INPUT.contains(&t));
        // A session first heard of mid-turn (e.g. tracking was just turned on) is working.
        let initial = match m.event.as_str() {
            "SessionStart" => Status::Idle,
            "Stop" => Status::Done,
            "Notification" if needs_input => Status::NeedsInput,
            _ => Status::Working,
        };
        let idx = self.index_or_insert(&m.session_id, initial, at);
        let s = &mut self.list[idx];
        s.hooked = true;
        s.updated_ms = s.updated_ms.max(at);
        if !m.cwd.is_empty() {
            s.cwd.clone_from(&m.cwd);
        }
        if m.title.is_some() {
            s.title.clone_from(&m.title);
        }
        if m.claude_pid.is_some() {
            s.claude_pid = m.claude_pid;
            s.claude_started = m.claude_started;
        }
        if m.ide_pid.is_some() {
            s.ide_pid = m.ide_pid;
        }
        if m.entrypoint.is_some() {
            s.entrypoint.clone_from(&m.entrypoint);
        }
        match m.event.as_str() {
            "UserPromptSubmit" => {
                if m.prompt.as_deref().is_some_and(crate::transcripts::is_user_text) {
                    s.last_prompt.clone_from(&m.prompt);
                }
                s.set_status(Status::Working, at);
            }
            "Notification" if needs_input => s.set_status(Status::NeedsInput, at),
            // Tool events run asynchronously and can arrive late, so they only
            // clear a pending prompt: they never undo "done".
            "PostToolUse" | "PostToolUseFailure" => {
                if s.status == Status::NeedsInput {
                    let waiting_since = s.since_ms;
                    s.set_status(Status::Working, at);
                    s.tool_cleared = Some((at, waiting_since));
                }
            }
            "Stop" => s.set_status(Status::Done, at),
            // A new process (started, resumed or cleared): agents of an earlier one are gone.
            "SessionStart" => s.agents.clear(),
            "SubagentStart" => {
                if let (Some(id), Some(kind)) = (&m.agent_id, &m.agent_type) {
                    // Claude Code's internal helpers report an empty type; skip them.
                    // A start that arrives after the transcript showed its end is late, not
                    // a new run; an agent continued after its end starts again later.
                    let known = s.agents.iter().any(|a| &a.id == id) || s.stats.agent_ended_since(id, at);
                    if !kind.is_empty() && !known {
                        s.agents.push(Agent { id: id.clone(), kind: kind.clone(), started_ms: at });
                    }
                }
            }
            "SubagentStop" => {
                if let Some(id) = &m.agent_id {
                    s.agents.retain(|a| &a.id != id);
                }
            }
            _ => {}
        }
    }

    /// Merges Claude Code's list of running sessions in. Returns whether
    /// anything changed.
    pub fn sync(&mut self, listed: &[Listed], at: i64) -> bool {
        let before = (self.list.clone(), self.hidden.len());
        // A process that moved on to another session (e.g. after /clear) no
        // longer runs the old one.
        self.list.retain(|s| {
            !listed.iter().any(|l| {
                Some(l.pid) == s.claude_pid && l.started == s.claude_started && l.distro == s.distro && l.id != s.id
            })
        });
        for l in listed {
            let initial = match l.state {
                ListedState::Busy => Status::Working,
                ListedState::Waiting => Status::NeedsInput,
                ListedState::Idle | ListedState::Unknown => Status::Idle,
            };
            let idx = self.index_or_insert(&l.id, initial, l.state_at_ms);
            let s = &mut self.list[idx];
            if !l.cwd.is_empty() {
                s.cwd.clone_from(&l.cwd);
            }
            s.name.clone_from(&l.name);
            s.distro.clone_from(&l.distro);
            s.claude_pid = Some(l.pid);
            s.claude_started = l.started;
            s.process_started_ms = l.started_ms;
            if s.entrypoint.is_none() {
                s.entrypoint.clone_from(&l.entrypoint);
            }
            s.updated_ms = s.updated_ms.max(l.state_at_ms);
            // A tool event cleared "needs you", but the list still says the
            // session waits, a while later: the event came from parallel work
            // and the prompt is still open.
            if let (ListedState::Waiting, Status::Working, Some((cleared, since))) = (l.state, s.status, s.tool_cleared)
            {
                if l.state_at_ms <= cleared && at - cleared >= STOP_GRACE_MS {
                    s.set_status(Status::NeedsInput, since);
                    s.restored = true;
                    continue;
                }
            }
            // Only news counts: what we know may be newer than the list.
            if l.state_at_ms <= s.since_ms {
                continue;
            }
            match (l.state, s.status) {
                (ListedState::Busy, Status::Working) | (ListedState::Waiting, Status::NeedsInput) => {}
                (ListedState::Busy, _) => s.set_status(Status::Working, l.state_at_ms),
                (ListedState::Waiting, _) => s.set_status(Status::NeedsInput, l.state_at_ms),
                (ListedState::Idle, Status::Working | Status::NeedsInput) => {
                    if !s.hooked {
                        // Without hooks, a turn that ends is a finished turn.
                        s.set_status(Status::Done, l.state_at_ms);
                    } else if at - l.state_at_ms >= STOP_GRACE_MS {
                        // No Stop hook came: the turn was interrupted.
                        s.set_status(Status::Idle, l.state_at_ms);
                        s.agents.clear();
                    }
                }
                _ => {}
            }
        }
        self.unhide_needing();
        (self.list.clone(), self.hidden.len()) != before
    }

    /// The hooks are gone (Instant updates switched off): no session waits for
    /// its `Stop` hook any more, so the end of a turn counts as done again
    /// rather than interrupted. A hook that still arrives marks its session anew.
    pub fn unhook(&mut self) {
        for s in &mut self.list {
            s.hooked = false;
        }
    }

    /// A hidden session that needs the user shows again.
    fn unhide_needing(&mut self) {
        let list = &self.list;
        self.hidden.retain(|id, _| !list.iter().any(|s| &s.id == id && s.status == Status::NeedsInput));
    }

    /// Drops WSL sessions that are gone: their distribution stopped, or it
    /// was read (`scanned`) and no longer lists them. Windows sessions are
    /// checked by process instead (see `retain_alive`).
    pub fn retain_wsl(&mut self, running: &[String], scanned: &[String], listed: &[Listed]) -> bool {
        let n = self.list.len();
        self.list.retain(|s| match &s.distro {
            None => true,
            Some(d) if !running.contains(d) => false,
            Some(d) if scanned.contains(d) => listed.iter().any(|l| l.id == s.id),
            Some(_) => true,
        });
        self.list.len() != n
    }

    /// Updates what the transcript says about a session. Returns whether it changed.
    pub fn set_stats(&mut self, id: &str, stats: Stats) -> bool {
        // The transcript shows a prompt the user wrote after hiding it: show it again.
        let prompted = |hidden_at: &i64| stats.last_prompt_at.is_some_and(|p| p * 1000 > *hidden_at);
        if self.hidden.get(id).is_some_and(prompted) {
            self.hidden.remove(id);
        }
        let Some(s) = self.list.iter_mut().find(|s| s.id == id) else { return false };
        // Agents whose SubagentStop was missed (Trayward wasn't running): the
        // transcript shows they ended.
        let before = s.agents.len();
        s.agents.retain(|a| !stats.agent_ended_since(&a.id, a.started_ms));
        let changed = s.agents.len() != before || s.stats != stats;
        s.stats = stats;
        changed
    }

    /// Marks the sessions in `archived` as archived, all others as not.
    /// Returns whether anything changed.
    pub fn set_archived(&mut self, archived: &HashSet<String>) -> bool {
        self.archived.clone_from(archived);
        let mut changed = false;
        for s in &mut self.list {
            let a = archived.contains(&s.id);
            if s.archived != a {
                s.archived = a;
                changed = true;
            }
        }
        changed
    }

    /// Updates the VS Code window name of a session. Returns whether it changed.
    pub fn set_workspace(&mut self, id: &str, workspace: Option<String>) -> bool {
        match self.list.iter_mut().find(|s| s.id == id) {
            Some(s) if s.workspace != workspace => {
                s.workspace = workspace;
                true
            }
            _ => false,
        }
    }

    /// Marks a finished session as seen. Returns whether it changed.
    pub fn mark_seen(&mut self, id: &str, at: i64) -> bool {
        match self.list.iter_mut().find(|s| s.id == id && s.status == Status::Done) {
            Some(s) => {
                s.set_status(Status::Idle, at);
                true
            }
            None => false,
        }
    }

    /// Hides a session until the user writes in it or it needs them; work it
    /// does on its own (messages from other sessions, background tasks) doesn't count.
    pub fn hide(&mut self, id: &str, at: i64) -> bool {
        let Some(s) = self.list.iter_mut().find(|s| s.id == id) else { return false };
        // Hiding a finished session also means it has been seen.
        if s.status == Status::Done {
            s.set_status(Status::Idle, at);
        }
        self.hidden.insert(id.to_string(), at);
        let list = &self.list;
        self.hidden.retain(|id, t| at - *t < HIDDEN_KEEP_MS || list.iter().any(|s| &s.id == id));
        true
    }

    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.contains_key(id)
    }

    /// Shows all hidden sessions again.
    pub fn unhide_all(&mut self) -> bool {
        let changed = self.hidden_count() > 0;
        self.hidden.clear();
        changed
    }

    /// Hidden sessions that are running.
    pub fn hidden_count(&self) -> usize {
        self.list.iter().filter(|s| self.is_hidden(&s.id)).count()
    }

    /// Drops sessions whose Claude Code process has exited.
    pub fn retain_alive(&mut self, alive: impl Fn(&Session) -> bool) -> bool {
        let n = self.list.len();
        self.list.retain(|s| alive(s));
        self.list.len() != n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(event: &str, at: i64) -> HookMessage {
        HookMessage {
            event: event.into(),
            session_id: "s1".into(),
            cwd: "C:\\code\\proj".into(),
            at_ms: at,
            ..Default::default()
        }
    }

    fn listed(state: ListedState, at: i64) -> Listed {
        Listed {
            id: "s1".into(),
            pid: 10,
            started: Some(99),
            cwd: "C:\\code\\proj".into(),
            name: Some("proj-1a".into()),
            state,
            state_at_ms: at,
            entrypoint: Some("claude-vscode".into()),
            distro: None,
            started_ms: None,
        }
    }

    fn status(s: &Sessions) -> Status {
        s.get("s1").unwrap().status
    }

    #[test]
    fn prompt_then_stop_is_done() {
        let mut s = Sessions::default();
        assert!(s.apply(&msg("SessionStart", 1)));
        assert_eq!(status(&s), Status::Idle);
        let mut p = msg("UserPromptSubmit", 2);
        p.prompt = Some("fix the bug".into());
        s.apply(&p);
        let x = s.get("s1").unwrap();
        assert_eq!((x.status, x.since_ms), (Status::Working, 2));
        assert_eq!(x.description(), Some("fix the bug"));
        s.apply(&msg("Stop", 3));
        assert_eq!(status(&s), Status::Done);
        assert_eq!(s.get("s1").unwrap().project(), "proj");
    }

    #[test]
    fn first_heard_mid_turn_is_working() {
        let mut s = Sessions::default();
        s.apply(&msg("PostToolUse", 1));
        assert_eq!(status(&s), Status::Working);
    }

    #[test]
    fn permission_prompt_needs_input_until_tool_runs() {
        let mut s = Sessions::default();
        s.apply(&msg("UserPromptSubmit", 1));
        let mut n = msg("Notification", 2);
        n.notification_type = Some("idle_prompt".into());
        s.apply(&n);
        assert_eq!(status(&s), Status::Working);
        n.notification_type = Some("permission_prompt".into());
        s.apply(&n);
        assert_eq!(status(&s), Status::NeedsInput);
        s.apply(&msg("PostToolUse", 3));
        assert_eq!(status(&s), Status::Working);
    }

    #[test]
    fn a_prompt_still_open_after_parallel_work_needs_the_user_again() {
        let mut s = Sessions::default();
        s.apply(&msg("UserPromptSubmit", 1_000));
        s.sync(&[listed(ListedState::Waiting, 2_000)], 2_000);
        assert_eq!(status(&s), Status::NeedsInput);
        // Another tool call, running in parallel, finishes: the hook can't tell whose it was.
        s.apply(&msg("PostToolUse", 3_000));
        assert_eq!(status(&s), Status::Working);
        // The list read right after may be older than the event: nothing yet.
        s.sync(&[listed(ListedState::Waiting, 2_000)], 3_500);
        assert_eq!(status(&s), Status::Working);
        // Still waiting a while later: the prompt is open, since when it was asked.
        s.sync(&[listed(ListedState::Waiting, 2_000)], 3_000 + STOP_GRACE_MS);
        assert_eq!((status(&s), s.list[0].since_ms), (Status::NeedsInput, 2_000));

        // The prompt was answered: the list says busy, and it stays working.
        s.apply(&msg("PostToolUse", 9_000));
        s.sync(&[listed(ListedState::Busy, 8_900)], 9_000 + STOP_GRACE_MS);
        assert_eq!(status(&s), Status::Working);
        s.sync(&[listed(ListedState::Busy, 8_900)], 60_000);
        assert_eq!(status(&s), Status::Working);
    }

    #[test]
    fn late_tool_event_does_not_undo_done() {
        let mut s = Sessions::default();
        s.apply(&msg("UserPromptSubmit", 1));
        s.apply(&msg("Stop", 3));
        s.apply(&msg("PostToolUse", 2));
        assert_eq!(status(&s), Status::Done);
    }

    #[test]
    fn tracks_subagents() {
        let mut s = Sessions::default();
        let mut a = msg("SubagentStart", 1);
        a.agent_id = Some("a1".into());
        a.agent_type = Some("Explore".into());
        s.apply(&a);
        s.apply(&a); // duplicates ignored
        let mut internal = a.clone();
        internal.agent_id = Some("a2".into());
        internal.agent_type = Some(String::new());
        s.apply(&internal);
        assert_eq!(s.get("s1").unwrap().agents.len(), 1);
        let mut stop = msg("SubagentStop", 2);
        stop.agent_id = Some("a1".into());
        s.apply(&stop);
        assert!(s.get("s1").unwrap().agents.is_empty());
    }

    #[test]
    fn the_transcript_ends_agents_whose_stop_was_missed() {
        let mut s = Sessions::default();
        let mut a = msg("SubagentStart", 1);
        a.agent_id = Some("a1".into());
        a.agent_type = Some("general-purpose".into());
        s.apply(&a);
        assert_eq!(s.get("s1").unwrap().running_agents(1), 1);
        let stats = Stats { agents_ended: vec![("a1".into(), 0)], ..Stats::default() };
        assert!(s.set_stats("s1", stats));
        assert_eq!(s.get("s1").unwrap().running_agents(2), 0);
        // A late start of an agent that already ended doesn't count it again.
        s.apply(&a);
        assert_eq!(s.get("s1").unwrap().running_agents(3), 0);
        // Continued later (a message to it), it runs again until it ends again.
        let mut again = msg("SubagentStart", 5_000);
        again.agent_id = Some("a1".into());
        again.agent_type = Some("general-purpose".into());
        s.apply(&again);
        assert_eq!(s.get("s1").unwrap().running_agents(5_001), 1);
        let still = Stats { agents_ended: vec![("a1".into(), 0)], output_tokens: 1, ..Stats::default() };
        s.set_stats("s1", still);
        assert_eq!(s.get("s1").unwrap().running_agents(5_002), 1);
        let ended = Stats { agents_ended: vec![("a1".into(), 9)], ..Stats::default() };
        s.set_stats("s1", ended);
        assert_eq!(s.get("s1").unwrap().running_agents(9_001), 0);
    }

    #[test]
    fn background_agents_outlive_the_turn_but_not_the_process() {
        let mut s = Sessions::default();
        s.apply(&msg("UserPromptSubmit", 1));
        let mut a = msg("SubagentStart", 2);
        a.agent_id = Some("a1".into());
        a.agent_type = Some("general-purpose".into());
        s.apply(&a);
        s.apply(&msg("Stop", 3));
        assert_eq!(status(&s), Status::Done);
        assert_eq!(s.get("s1").unwrap().running_agents(3), 1);
        s.apply(&msg("SessionStart", 4));
        assert_eq!(s.get("s1").unwrap().running_agents(4), 0);
    }

    #[test]
    fn counts_agents_from_the_transcript() {
        let mut s = Session::new("s1", Status::Done, 0);
        let agent = |until| Watcher { kind: WatchKind::Agent, label: "Review".into(), since: 100, until };
        s.stats.agent_calls = vec![100];
        s.stats.watchers = vec![
            agent(None),
            agent(Some(150)),
            Watcher { kind: WatchKind::Command, label: "Build".into(), since: 100, until: None },
        ];
        assert_eq!(s.running_agents(120_000), 3);
        assert_eq!(s.running_agents(200_000), 2);
        // The process was started again since: what it waited for is gone.
        s.process_started_ms = Some(180_000);
        assert_eq!(s.running_agents(200_000), 0);
    }

    #[test]
    fn session_end_removes() {
        let mut s = Sessions::default();
        s.apply(&msg("SessionStart", 1));
        assert!(s.apply(&msg("SessionEnd", 2)));
        assert!(s.get("s1").is_none());
        assert!(!s.apply(&msg("SessionEnd", 3)));
    }

    #[test]
    fn listed_sessions_appear_with_their_state() {
        let mut s = Sessions::default();
        assert!(s.sync(&[listed(ListedState::Busy, 5)], 6));
        let x = s.get("s1").unwrap();
        assert_eq!((x.status, x.since_ms, x.claude_pid), (Status::Working, 5, Some(10)));
        assert_eq!(x.description(), Some("proj-1a"));
        assert!(x.in_vscode());
        s.sync(&[listed(ListedState::Waiting, 7)], 8);
        assert_eq!(status(&s), Status::NeedsInput);
        // Without hooks, going idle after work means done.
        s.sync(&[listed(ListedState::Idle, 9)], 10);
        assert_eq!(status(&s), Status::Done);
        // The same listing again changes nothing.
        assert!(!s.sync(&[listed(ListedState::Idle, 9)], 11));
    }

    #[test]
    fn idle_listing_of_a_new_session_is_idle() {
        let mut s = Sessions::default();
        s.sync(&[listed(ListedState::Idle, 5)], 6);
        assert_eq!(status(&s), Status::Idle);
    }

    #[test]
    fn hooked_session_waits_for_stop() {
        let mut s = Sessions::default();
        s.apply(&msg("UserPromptSubmit", 1));
        s.sync(&[listed(ListedState::Idle, 10)], 11);
        assert_eq!(status(&s), Status::Working); // Stop may still be on its way
        s.apply(&msg("Stop", 12));
        assert_eq!(status(&s), Status::Done);
    }

    #[test]
    fn hooked_session_without_stop_was_interrupted() {
        let mut s = Sessions::default();
        s.apply(&msg("UserPromptSubmit", 1));
        s.sync(&[listed(ListedState::Idle, 10)], 10 + STOP_GRACE_MS);
        assert_eq!(status(&s), Status::Idle);
    }

    #[test]
    fn stale_listing_does_not_override_newer_hooks() {
        let mut s = Sessions::default();
        s.apply(&msg("UserPromptSubmit", 1));
        s.apply(&msg("Stop", 20));
        s.sync(&[listed(ListedState::Busy, 15)], 21);
        assert_eq!(status(&s), Status::Done);
        // A new turn the list saw first does count.
        s.sync(&[listed(ListedState::Busy, 30)], 31);
        assert_eq!(status(&s), Status::Working);
    }

    #[test]
    fn process_that_moved_on_drops_old_session() {
        let mut s = Sessions::default();
        s.sync(&[listed(ListedState::Idle, 5)], 6);
        let mut cleared = listed(ListedState::Idle, 7);
        cleared.id = "s2".into();
        s.sync(&[cleared], 8);
        assert!(s.get("s1").is_none());
        assert!(s.get("s2").is_some());
    }

    #[test]
    fn wsl_sessions_follow_their_distro() {
        let mut s = Sessions::default();
        let mut wsl = listed(ListedState::Idle, 5);
        wsl.distro = Some("Ubuntu".into());
        s.sync(
            &[wsl.clone(), listed(ListedState::Idle, 5)].map(|mut l| {
                if l.distro.is_none() {
                    l.id = "win".into();
                    l.pid = 11;
                }
                l
            }),
            6,
        );
        let running = ["Ubuntu".to_string()];
        // A failed read of the distribution keeps its sessions.
        assert!(!s.retain_wsl(&running, &[], &[]));
        // Read and still listed: kept.
        assert!(!s.retain_wsl(&running, &running, &[wsl.clone()]));
        // Read and gone: dropped. Windows sessions are untouched.
        assert!(s.retain_wsl(&running, &running, &[]));
        assert!(s.get("s1").is_none());
        assert!(s.get("win").is_some());
        // A Windows process with the same pid doesn't push out a WSL session.
        s.sync(&[wsl.clone()], 7);
        let mut other = listed(ListedState::Idle, 8);
        other.id = "s2".into();
        s.sync(&[wsl, other], 9);
        assert!(s.get("s1").is_some());
        assert!(s.retain_wsl(&[], &[], &[]));
        assert!(s.get("s1").is_none());
    }

    #[test]
    fn marks_archived_sessions() {
        let mut s = Sessions::default();
        s.apply(&msg("SessionStart", 1));
        let ids: HashSet<String> = ["s1".to_string(), "s2".to_string()].into();
        assert!(s.set_archived(&ids));
        assert!(s.get("s1").unwrap().archived);
        assert!(!s.set_archived(&ids));
        // A session that appears later is archived from the start.
        let mut m = msg("SessionStart", 2);
        m.session_id = "s2".into();
        s.apply(&m);
        assert!(s.get("s2").unwrap().archived);
        assert!(s.set_archived(&Default::default()));
        assert!(!s.get("s1").unwrap().archived);
    }

    #[test]
    fn hiding_until_the_user_writes() {
        let mut s = Sessions::default();
        s.apply(&msg("Stop", 1));
        assert!(s.hide("s1", 2));
        assert!(s.is_hidden("s1"));
        assert_eq!(status(&s), Status::Idle);
        assert_eq!(s.hidden_count(), 1);
        // Another session's message starts a turn: still hidden.
        let mut peer = msg("UserPromptSubmit", 3);
        peer.prompt = Some("Another Claude session sent a message: <cross-session-message>".into());
        s.apply(&peer);
        s.apply(&msg("Stop", 4));
        assert!(s.is_hidden("s1"));
        // So is a turn seen only in the session list.
        s.sync(&[listed(ListedState::Busy, 5)], 5);
        s.sync(&[listed(ListedState::Idle, 6)], 6 + STOP_GRACE_MS);
        assert!(s.is_hidden("s1"));
        // The user's own prompt shows it.
        let mut p = msg("UserPromptSubmit", 7);
        p.prompt = Some("fix the bug".into());
        s.apply(&p);
        assert!(!s.is_hidden("s1"));
        s.hide("s1", 8);
        assert!(s.unhide_all());
        assert_eq!(s.hidden_count(), 0);
    }

    #[test]
    fn hidden_until_a_prompt_in_the_transcript_or_a_question() {
        let mut s = Sessions::default();
        s.sync(&[listed(ListedState::Idle, 1)], 1);
        s.hide("s1", 10_000);
        // A prompt from before hiding changes nothing; a newer one shows it.
        s.set_stats("s1", Stats { last_prompt_at: Some(9), ..Default::default() });
        assert!(s.is_hidden("s1"));
        s.set_stats("s1", Stats { last_prompt_at: Some(11), ..Default::default() });
        assert!(!s.is_hidden("s1"));
        // Needing the user shows it too.
        s.hide("s1", 20_000);
        s.sync(&[listed(ListedState::Waiting, 20_001)], 20_001);
        assert!(!s.is_hidden("s1"));
    }

    #[test]
    fn hidden_sessions_stay_hidden_when_resumed() {
        let mut s = Sessions::default();
        s.sync(&[listed(ListedState::Idle, 1)], 1);
        s.hide("s1", 2);
        assert!(s.retain_alive(|_| false));
        assert_eq!(s.hidden_count(), 0);
        s.sync(&[listed(ListedState::Idle, 3)], 3);
        assert!(s.is_hidden("s1"));
        assert_eq!(s.hidden_count(), 1);
    }

    #[test]
    fn seen_and_recently_done() {
        let mut s = Sessions::default();
        for (id, ev, at) in [("a", "Stop", 1), ("b", "UserPromptSubmit", 2), ("c", "Stop", 3)] {
            let mut m = msg(ev, at);
            m.session_id = id.into();
            s.apply(&m);
        }
        assert!(s.mark_seen("c", 4));
        // Seen, but still recently done.
        let c = s.get("c").unwrap();
        assert!(c.recently_done(5, 10) && !c.recently_done(20, 10));
        assert!(!s.get("b").unwrap().recently_done(5, 10));
        assert!(!s.mark_seen("b", 4));
    }

    #[test]
    fn description_prefers_transcript_title() {
        let mut s = Sessions::default();
        let mut p = msg("UserPromptSubmit", 1);
        p.prompt = Some("prompt".into());
        s.apply(&p);
        let stats = Stats { title: Some("Nice title".into()), ..Default::default() };
        assert!(s.set_stats("s1", stats.clone()));
        assert!(!s.set_stats("s1", stats));
        assert_eq!(s.get("s1").unwrap().description(), Some("Nice title"));
    }

    #[test]
    fn project_name_from_cwd() {
        let mut x = Session::new("x", Status::Idle, 0);
        x.cwd = "c:\\dev\\tools\\my-app\\".into();
        assert_eq!(x.project(), "my-app");
        x.cwd = String::new();
        assert_eq!(x.project(), "Claude");
    }
}
