//! Entry points for the fuzzer (`fuzz/`, cargo-fuzz): each takes raw bytes, as a file or
//! the network would hand them over, and runs the parser that reads them. They return
//! nothing: what counts is that no input panics (AGENTS.md: parse defensively, never
//! fail or panic on what isn't understood). Behind the `fuzzing` feature, never in a
//! build of the app.

use serde_json::Value;

/// The bytes as text, the way the readers get it: invalid UTF-8 replaced.
fn text(data: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(data)
}

/// Claude Code's transcript lines: the session tracker's parser, the statistics (also
/// as Copilot's events) and what a session loads before its first reply.
pub fn transcript(data: &[u8]) {
    let text = text(data);
    let mut parser = crate::transcripts::Parser::default();
    let mut stats = crate::stats::Stats::default();
    let mut copilot = crate::stats::Stats::default();
    let mut setup = crate::setup::Setup::default();
    for (i, line) in text.lines().enumerate() {
        parser.line(line);
        stats.line(line);
        copilot.copilot_line(line);
        setup.line(line, i < 8);
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            let _ = crate::transcripts::user_prompt(&v);
        }
    }
}

/// A session list file (`~/.claude/sessions/*.json`), and a hook's input.
pub fn session(data: &[u8]) {
    let Ok(v) = serde_json::from_slice::<Value>(data) else { return };
    let _ = crate::registry::parse(&v, None);
    let _ = crate::registry::parse(&v, Some("Ubuntu"));
    let _ = crate::hook::HookMessage::from_input(&v, crate::hook::Origin::default(), 0);
    let _ = crate::tracking::state(&v, "trayward.exe");
}

/// The usage endpoint's and the profile's answers, and Claude Code's saved login.
pub fn usage(data: &[u8]) {
    let _ = crate::usage::parse_usage(data);
    let _ = crate::usage::parse_plan(data);
    let _ = crate::usage::access_token(&text(data), 0);
}

/// An extension's JSON file.
pub fn extension(data: &[u8]) {
    let _ = crate::extensions::parse(&text(data), 0);
}

/// Codex's and Copilot's session files, line by line.
pub fn agent_lines(data: &[u8]) {
    let text = text(data);
    let mut codex = crate::codex::Parsed::default();
    let mut copilot = crate::copilot::Parsed::default();
    for (i, line) in text.lines().enumerate() {
        crate::codex::parse_line(&mut codex, line, i == 0);
        crate::copilot::parse_line(&mut copilot, line);
    }
}

/// The state file (`state.json`), damaged or not.
pub fn state(data: &[u8]) {
    let _ = crate::store::parse(data);
}

/// What the user types or pastes: broker and push addresses; and versions and times
/// read from GitHub and the agents' files.
pub fn addresses(data: &[u8]) {
    let text = text(data);
    let _ = crate::mqtt::parse(&text);
    let _ = crate::push::Config::from_address(&text);
    let _ = crate::push::Config::load(&text);
    let _ = crate::update::version(&text);
    let _ = crate::timefmt::parse_rfc3339(&text);
    let _ = crate::sessions::ListedState::parse(&text);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry point takes a few odd inputs, the way the fuzzer starts.
    #[test]
    fn takes_anything() {
        let inputs: [&[u8]; 6] = [
            b"",
            b"\xff\xfe not text",
            b"{\"type\":\"assistant\",\"timestamp\":\"2026-11-22T10:00:00Z\",\"message\":{\"id\":\"m\",\"usage\":{}}}",
            b"{\"sessionId\":\"0b5c\",\"pid\":-1,\"cwd\":7}\n{\"hook_event_name\":\"Stop\"}",
            b"mqtts://user:pa%ss@[::1]:99999999",
            b"{\"version\":1,\"rows\":[{\"type\":\"progress\",\"value\":-1e999}]}",
        ];
        for data in inputs {
            transcript(data);
            session(data);
            usage(data);
            extension(data);
            agent_lines(data);
            state(data);
            addresses(data);
        }
    }
}
