//! What a session starts with: the skills, MCP servers, agent types,
//! instruction files and memory Claude Code loads, and how much of the
//! context they take before Claude answers the first prompt.
//!
//! Claude Code writes them into the transcript as `attachment` lines at the
//! start, and again when something changes: `skill_listing`,
//! `deferred_tools_delta` (tool names; MCP tools are `mcp__<server>__<tool>`),
//! `mcp_instructions_delta`, `agent_listing_delta`, `instructions` (CLAUDE.md,
//! AGENTS.md; `AutoMem` is auto memory's index, MEMORY.md) and
//! `prompt_snapshot` (the system prompt). The counts follow every change; the
//! sizes are those before the first reply, in characters (a token is about
//! four). The built-in tools' definitions aren't in the transcript: they are
//! in the rest, the first reply's context minus the listed parts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// Characters per token, roughly, for estimates.
pub const CHARS_PER_TOKEN: u64 = 4;

/// The attachment types read here; other attachments are skipped unparsed.
const TYPES: &[&str] = &[
    "\"type\":\"skill_listing\"",
    "\"type\":\"deferred_tools_delta\"",
    "\"type\":\"mcp_instructions_delta\"",
    "\"type\":\"agent_listing_delta\"",
    "\"type\":\"instructions\"",
    "\"type\":\"prompt_snapshot\"",
];

/// Characters of each part loaded before the first reply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chars {
    pub system: u64,
    pub skills: u64,
    pub mcp: u64,
    pub agents: u64,
    pub instructions: u64,
    pub memory: u64,
}

/// A part of the starting context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    System,
    Skills,
    Mcp,
    Agents,
    Instructions,
    Memory,
    /// Everything else: the built-in tools' definitions, the rest of the
    /// system prompt, the first prompt.
    Rest,
}

impl Part {
    pub fn label(self) -> &'static str {
        match self {
            Part::System => "System prompt",
            Part::Skills => "Skills",
            Part::Mcp => "MCP servers",
            Part::Agents => "Agent types",
            Part::Instructions => "CLAUDE.md and rules",
            Part::Memory => "Memory",
            Part::Rest => "Tools and the rest",
        }
    }
}

/// What a session loads (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Setup {
    /// Any of it was in the transcript (older Claude Code versions wrote none).
    pub known: bool,
    pub skills: u32,
    pub mcp_servers: u32,
    pub mcp_tools: u32,
    pub agents: u32,
    /// CLAUDE.md, AGENTS.md and other instruction files, not memory.
    pub instruction_files: u32,
    /// Auto memory's index (MEMORY.md) was loaded.
    pub memory: bool,
    pub chars: Chars,
    #[serde(skip)]
    skill_names: BTreeSet<String>,
    #[serde(skip)]
    tools: BTreeSet<String>,
    /// Servers named by their instructions (others only by their tools).
    #[serde(skip)]
    servers: BTreeSet<String>,
    #[serde(skip)]
    agent_types: BTreeSet<String>,
}

fn strings(v: &Value) -> impl Iterator<Item = &str> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str)
}

fn chars(v: &Value) -> u64 {
    strings(v).map(|s| s.chars().count() as u64).sum()
}

impl Setup {
    /// Takes in a transcript line; `starting`: no reply yet, so sizes count.
    pub fn line(&mut self, line: &str, starting: bool) {
        if !TYPES.iter().any(|t| line.contains(t)) {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        if v["type"] != "attachment" {
            return;
        }
        let a = &v["attachment"];
        let initial = a["isInitial"] == true;
        match a["type"].as_str() {
            Some("skill_listing") => {
                if initial {
                    self.skill_names.clear();
                }
                self.skill_names.extend(strings(&a["names"]).map(str::to_string));
                let listed = a["skillCount"].as_u64().unwrap_or(0) as u32;
                self.skills = (self.skill_names.len() as u32).max(listed);
                if starting {
                    let n = a["content"].as_str().map_or(0, |s| s.chars().count() as u64);
                    self.chars.skills = if initial { n } else { self.chars.skills + n };
                }
            }
            Some("deferred_tools_delta") => {
                for name in strings(&a["removedNames"]) {
                    self.tools.remove(name);
                }
                self.tools.extend(strings(&a["addedNames"]).map(str::to_string));
                if starting {
                    // One listing line per tool; MCP tools' lines count for MCP.
                    let lines = strings(&a["addedLines"]);
                    self.chars.mcp += strings(&a["addedNames"])
                        .zip(lines)
                        .filter(|(n, _)| n.starts_with("mcp__"))
                        .map(|(_, l)| l.chars().count() as u64)
                        .sum::<u64>();
                }
            }
            Some("mcp_instructions_delta") => {
                for name in strings(&a["removedNames"]) {
                    self.servers.remove(name);
                }
                self.servers.extend(strings(&a["addedNames"]).map(str::to_string));
                if starting {
                    self.chars.mcp += chars(&a["addedBlocks"]);
                }
            }
            Some("agent_listing_delta") => {
                if initial {
                    self.agent_types.clear();
                }
                for name in strings(&a["removedTypes"]) {
                    self.agent_types.remove(name);
                }
                self.agent_types.extend(strings(&a["addedTypes"]).map(str::to_string));
                self.agents = self.agent_types.len() as u32;
                if starting {
                    self.chars.agents += chars(&a["addedLines"]);
                }
            }
            Some("instructions") => {
                let files: Vec<&Value> = a["files"].as_array().into_iter().flatten().collect();
                let memory = |f: &&&Value| f["type"] == "AutoMem";
                self.instruction_files = files.iter().filter(|f| !memory(f)).count() as u32;
                self.memory = files.iter().any(|f| memory(&f));
                if starting {
                    let size = |f: &&Value| f["content"].as_str().map_or(0, |s| s.chars().count() as u64);
                    self.chars.instructions = files.iter().filter(|f| !memory(f)).map(size).sum();
                    self.chars.memory = files.iter().filter(|f| memory(f)).map(size).sum();
                }
            }
            Some("prompt_snapshot") if starting => self.chars.system = chars(&a["systemPrompt"]),
            _ => return,
        }
        self.known = true;
        let mcp: Vec<&String> = self.tools.iter().filter(|t| t.starts_with("mcp__")).collect();
        self.mcp_tools = mcp.len() as u32;
        let mut servers = self.servers.clone();
        servers.extend(mcp.iter().filter_map(|t| t.split("__").nth(1)).map(str::to_string));
        self.mcp_servers = servers.len() as u32;
    }

    /// The parts of a starting context of `context` tokens: estimates of the
    /// listed parts, then the rest. Empty when nothing is known.
    pub fn parts(&self, context: u64) -> Vec<(Part, u64)> {
        if !self.known || context == 0 {
            return Vec::new();
        }
        let c = self.chars;
        let mut parts: Vec<(Part, u64)> = [
            (Part::System, c.system),
            (Part::Skills, c.skills),
            (Part::Mcp, c.mcp),
            (Part::Agents, c.agents),
            (Part::Instructions, c.instructions),
            (Part::Memory, c.memory),
        ]
        .into_iter()
        .map(|(p, n)| (p, n / CHARS_PER_TOKEN))
        .collect();
        let listed: u64 = parts.iter().map(|p| p.1).sum();
        parts.push((Part::Rest, context.saturating_sub(listed)));
        parts
    }
}

/// Memory files of a project folder (`<config>/projects/<folder>/memory`),
/// not counting the index.
pub fn memory_files(project_folder: &Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(project_folder.join("memory")) else { return 0 };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            name.ends_with(".md") && name != "memory.md"
        })
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(json: &str) -> String {
        format!(r#"{{"type":"attachment","attachment":{json}}}"#)
    }

    #[test]
    fn counts_and_sizes_what_a_session_loads() {
        let mut s = Setup::default();
        s.line(
            &attachment(
                r#"{"type":"skill_listing","content":"12345678","skillCount":2,"isInitial":true,"names":["a","b"]}"#,
            ),
            true,
        );
        s.line(&attachment(r#"{"type":"deferred_tools_delta","addedNames":["mcp__tracker__search","mcp__tracker__get","mcp__sonar__scan","WebFetch"],"addedLines":["1234","1234","1234","123456789"],"removedNames":[]}"#), true);
        s.line(&attachment(r#"{"type":"mcp_instructions_delta","addedNames":["docs"],"addedBlocks":["12345678"],"removedNames":[]}"#), true);
        s.line(&attachment(r#"{"type":"agent_listing_delta","addedTypes":["Explore","Plan"],"addedLines":["1234","1234"],"isInitial":true}"#), true);
        s.line(&attachment(r#"{"type":"instructions","files":[{"path":"C:/u/CLAUDE.md","type":"User","content":"12345678"},{"path":"p/AGENTS.md","type":"Project","content":"1234"},{"path":"m/MEMORY.md","type":"AutoMem","content":"1234"}]}"#), true);
        s.line(&attachment(r#"{"type":"prompt_snapshot","systemPrompt":["12345678","1234"]}"#), true);
        // Other attachments are skipped.
        s.line(&attachment(r#"{"type":"deferred_tools_record","names":["x"]}"#), true);
        assert!(s.known);
        assert_eq!((s.skills, s.mcp_servers, s.mcp_tools, s.agents), (2, 3, 3, 2));
        assert_eq!((s.instruction_files, s.memory), (2, true));
        assert_eq!(s.chars, Chars { system: 12, skills: 8, mcp: 20, agents: 8, instructions: 12, memory: 4 });
        let parts = s.parts(100);
        assert_eq!(parts.first(), Some(&(Part::System, 3)));
        assert_eq!(parts.last(), Some(&(Part::Rest, 100 - 3 - 2 - 5 - 2 - 3 - 1)));

        // After the first reply, changes count but sizes stay those of the start.
        s.line(&attachment(r#"{"type":"deferred_tools_delta","addedNames":["mcp__git__log"],"addedLines":["1234"],"removedNames":["mcp__sonar__scan"]}"#), false);
        s.line(&attachment(r#"{"type":"skill_listing","content":"1234","skillCount":1,"names":["c"]}"#), false);
        assert_eq!((s.skills, s.mcp_servers, s.mcp_tools), (3, 3, 3));
        assert_eq!(s.chars.mcp, 20);
    }

    #[test]
    fn nothing_known_without_attachments() {
        let mut s = Setup::default();
        s.line(r#"{"type":"user","message":{"content":"hi"}}"#, true);
        s.line("garbage \"type\":\"instructions\"", true);
        assert!(!s.known);
        assert!(s.parts(50_000).is_empty());
    }

    #[test]
    fn counts_memory_files() {
        let dir = std::env::temp_dir().join(format!("trayward-memory-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("memory")).unwrap();
        for name in ["MEMORY.md", "a.md", "b.md", "notes.txt"] {
            std::fs::write(dir.join("memory").join(name), "x").unwrap();
        }
        assert_eq!(memory_files(&dir), 2);
        assert_eq!(memory_files(&dir.join("missing")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
