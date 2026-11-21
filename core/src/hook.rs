//! The `trayward-hook` subcommand. Claude Code runs it for each hook event with the
//! event as JSON on stdin. It keeps only the fields the app needs, adds which
//! process the session runs in, and hands the result to the running app.
//!
//! It always exits 0 and prints nothing, so it never changes what Claude does.
//! How the message reaches the app is the app's (on Windows a `WM_COPYDATA` to
//! its control window): [`run`] hands it the bytes.

use crate::os;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;

/// Argument that selects hook mode. Also how our entries in Claude Code's
/// settings.json are recognised.
pub const ARG: &str = "trayward-hook";

/// Largest message the app accepts.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// Longest prompt excerpt kept, in characters.
const PROMPT_CHARS: usize = 160;

/// Hook input can include whole tool outputs; anything beyond this is dropped.
const MAX_INPUT: u64 = 32 * 1024 * 1024;

/// Whether `id` can be a session's id as Claude Code writes them (letters, digits,
/// `-` and `_`). Ids come from outside (a hook's input, a session list, any
/// program's message to the app) and become part of a file's path and of the
/// bubble's target: one that is a path or names an address isn't taken.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One hook event, reduced to what the app uses.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HookMessage {
    pub event: String,
    pub session_id: String,
    pub cwd: String,
    pub at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_started: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ide_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
}

/// Where the hook runs: the Claude Code process and its environment.
#[derive(Default)]
pub struct Origin {
    pub claude_pid: Option<u32>,
    pub claude_started: Option<u64>,
    pub ide_pid: Option<u32>,
    pub entrypoint: Option<String>,
}

impl HookMessage {
    /// Builds a message from Claude Code's hook input. `None` if it isn't one.
    pub fn from_input(input: &Value, origin: Origin, at_ms: i64) -> Option<HookMessage> {
        let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
        let event = text("hook_event_name").filter(|e| !e.is_empty())?;
        let session_id = text("session_id").filter(|s| valid_session_id(s))?;
        // In a subagent, agent_id/agent_type describe the subagent the event comes
        // from; only the subagent lifecycle events are about agents for us.
        let agent_event = matches!(event.as_str(), "SubagentStart" | "SubagentStop");
        Some(HookMessage {
            cwd: text("cwd").unwrap_or_default(),
            title: text("session_title").filter(|t| !t.trim().is_empty()),
            prompt: text("prompt").map(|p| excerpt(&p, PROMPT_CHARS)).filter(|p| !p.is_empty()),
            notification_type: text("notification_type"),
            agent_id: if agent_event { text("agent_id") } else { None },
            agent_type: if agent_event { text("agent_type") } else { None },
            claude_pid: origin.claude_pid,
            claude_started: origin.claude_started,
            ide_pid: origin.ide_pid,
            entrypoint: origin.entrypoint,
            event,
            session_id,
            at_ms,
        })
    }
}

/// First line-ish of a prompt: whitespace collapsed, at most `max` characters.
fn excerpt(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Shells that may sit between Claude Code and this process.
const SHELLS: &[&str] = &["bash.exe", "sh.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "env.exe"];

/// Finds the Claude Code process: our nearest ancestor that isn't a shell.
fn claude_process() -> Option<u32> {
    let table = os::process_table();
    let mut pid = table.get(&std::process::id())?.parent;
    for _ in 0..8 {
        let entry = table.get(&pid)?;
        if !SHELLS.iter().any(|s| entry.exe.eq_ignore_ascii_case(s)) {
            return Some(pid);
        }
        pid = entry.parent;
    }
    None
}

fn origin() -> Origin {
    let claude_pid = claude_process();
    Origin {
        claude_started: claude_pid.and_then(os::process_start_time),
        claude_pid,
        ide_pid: std::env::var("VSCODE_PID").ok().and_then(|p| p.parse().ok()),
        entrypoint: std::env::var("CLAUDE_CODE_ENTRYPOINT").ok().filter(|e| !e.is_empty()),
    }
}

/// How long hook mode may run at most.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// Runs hook mode. Never fails: problems are swallowed so Claude Code is unaffected.
pub fn run(deliver: impl FnOnce(&[u8])) -> i32 {
    // Never linger: if stdin isn't closed or the app doesn't answer, give up.
    std::thread::spawn(|| {
        std::thread::sleep(DEADLINE);
        std::process::exit(0);
    });
    // One JSON value is all there is: stop after it rather than wait for the
    // end of stdin, which Claude Code may leave open.
    let input = std::io::stdin().take(MAX_INPUT);
    let Some(Ok(value)) = serde_json::Deserializer::from_reader(input).into_iter::<Value>().next() else { return 0 };
    let Some(msg) = HookMessage::from_input(&value, origin(), os::now_ms()) else { return 0 };
    match serde_json::to_vec(&msg) {
        Ok(bytes) if bytes.len() <= MAX_MESSAGE => deliver(&bytes),
        _ => {}
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_message_from_hook_input() {
        let input = json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "abc",
            "cwd": "C:\\p",
            "prompt": "  fix\n\n the   bug  ",
            "session_title": "Fix bug",
            "agent_id": "inside-subagent"
        });
        let m = HookMessage::from_input(&input, Origin::default(), 5).unwrap();
        assert_eq!(m.event, "UserPromptSubmit");
        assert_eq!(m.prompt.as_deref(), Some("fix the bug"));
        assert_eq!(m.title.as_deref(), Some("Fix bug"));
        assert_eq!(m.agent_id, None);
        assert_eq!(m.at_ms, 5);
    }

    #[test]
    fn keeps_agent_fields_for_subagent_events() {
        let input =
            json!({"hook_event_name": "SubagentStart", "session_id": "s", "agent_id": "a", "agent_type": "Explore"});
        let m = HookMessage::from_input(&input, Origin::default(), 0).unwrap();
        assert_eq!((m.agent_id.as_deref(), m.agent_type.as_deref()), (Some("a"), Some("Explore")));
    }

    #[test]
    fn rejects_non_hook_input() {
        assert!(HookMessage::from_input(&json!({}), Origin::default(), 0).is_none());
        assert!(HookMessage::from_input(&json!({"hook_event_name": "Stop"}), Origin::default(), 0).is_none());
        assert!(HookMessage::from_input(&json!([1, 2]), Origin::default(), 0).is_none());
    }

    #[test]
    fn takes_only_ids_that_are_ids() {
        assert!(valid_session_id("0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"));
        assert!(valid_session_id("demo_1"));
        // A path (it would replace the folder it's joined to), an address, the bubble's own targets.
        for bad in ["", r"\\host\share\x", "C:/x", "../x", "url:https://example.com", "stats:", "a b", &"a".repeat(129)]
        {
            assert!(!valid_session_id(bad), "{bad}");
            let input = json!({"hook_event_name": "Stop", "session_id": bad});
            assert!(HookMessage::from_input(&input, Origin::default(), 0).is_none(), "{bad}");
        }
    }

    #[test]
    fn excerpts_long_prompts() {
        assert_eq!(excerpt("abc", 5), "abc");
        assert_eq!(excerpt("abcdefgh", 5), "abcd…");
        assert_eq!(excerpt("äöüßéèê", 4).chars().count(), 4);
    }

    #[test]
    fn message_round_trips_small() {
        let m = HookMessage { event: "Stop".into(), session_id: "s".into(), ..Default::default() };
        let bytes = serde_json::to_vec(&m).unwrap();
        assert!(bytes.len() < 100);
        assert_eq!(serde_json::from_slice::<HookMessage>(&bytes).unwrap(), m);
    }
}
