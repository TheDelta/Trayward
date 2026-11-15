//! Demo mode (`trayward.exe --demo`): made-up projects, sessions, usage and
//! history, for trying Trayward without Claude Code and for screenshots that
//! show nothing private.
//!
//! It writes a small Claude Code config folder to the temp folder in the
//! formats Claude Code uses (session list, transcripts), so the app reads it
//! exactly like the real thing. Nothing of the real setup is read or changed:
//! no login, no network, no saved state, no hooks, no WSL, no GPG.

use crate::forecast::{self, Sample};
use crate::timefmt::rfc3339;
use crate::usage::{Limit, Usage};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Demo sessions use made-up process ids from here on; they count as running.
pub const PID_BASE: u32 = 4_000_000;

const MINUTE: i64 = 60;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Whether the app runs in demo mode.
pub fn active() -> bool {
    DIR.get().is_some()
}

/// The demo's Claude Code config folder.
pub fn dir() -> Option<&'static PathBuf> {
    DIR.get()
}

/// Weekly usage now, and when it resets.
const WEEKLY_USED: f64 = 46.0;
const WEEKLY_RESETS_IN: i64 = 2 * DAY + 7 * HOUR;

/// The usage the demo shows instead of fetching it. Ultra has no 5-hour limit.
pub fn usage(now: i64) -> Usage {
    let five_hour = (plan() != "Ultra").then(|| Limit { used: 31.0, resets_at: Some(now + 2 * HOUR + 10 * MINUTE) });
    Usage { weekly: Some(Limit { used: WEEKLY_USED, resets_at: Some(now + WEEKLY_RESETS_IN) }), five_hour }
}

/// The plans the demo can show (right-click → Demo plan), as the usage service names them.
pub const PLANS: [&str; 2] = ["Max", "Ultra"];
static PLAN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The plan the demo shows now.
pub fn plan() -> &'static str {
    PLANS[PLAN.load(std::sync::atomic::Ordering::Relaxed).min(PLANS.len() - 1)]
}

pub fn set_plan(i: usize) {
    PLAN.store(i.min(PLANS.len() - 1), std::sync::atomic::Ordering::Relaxed);
}

/// Weekly usage readings: this week so far and the two weeks before.
pub fn history(now: i64) -> Vec<Sample> {
    let reset = now + WEEKLY_RESETS_IN;
    let mut out = Vec::new();
    for (weeks_ago, end) in [(2, 88.0), (1, 71.0), (0, WEEKLY_USED)] {
        let r = reset - weeks_ago * forecast::WEEK;
        let start = r - forecast::WEEK;
        let last = if weeks_ago == 0 { now } else { r - HOUR };
        // Use grows during the day and pauses at night.
        let steps: Vec<i64> = (0..).map(|i| start + i * 3 * HOUR).take_while(|t| *t <= last).collect();
        let weight = |t: i64| {
            let hour = t.rem_euclid(DAY) / HOUR;
            if (8..20).contains(&hour) { 1.0 } else { 0.15 }
        };
        let total: f64 = steps.iter().map(|t| weight(*t)).sum::<f64>().max(1.0);
        let mut used = 0.0;
        for t in steps {
            used += end * weight(t) / total;
            out.push(Sample { at: t, used: (used * 10.0).round() / 10.0, resets_at: Some(r), plan: None });
        }
    }
    out
}

// ---------- generating the files ----------

/// A small deterministic random generator, so every demo looks the same.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1).max(1) as u64) as i64
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.next() as usize % items.len()]
    }
}

struct Project {
    folder: &'static str,
    /// VS Code window title, so the window name shows like a real one.
    window: &'static str,
    files: &'static [&'static str],
    commands: &'static [&'static str],
    prompts: &'static [&'static str],
}

const PROJECTS: &[Project] = &[
    Project {
        folder: "shop",
        window: "🛍️ Shop",
        files: &[
            "src/checkout/Cart.tsx",
            "src/checkout/Payment.tsx",
            "src/api/orders.ts",
            "src/styles/checkout.css",
            "README.md",
        ],
        commands: &[
            "npm test",
            "npm run lint",
            "git status",
            "git diff --stat",
            "npx playwright test checkout",
            "npm run build",
            "git commit -m \"Fix the checkout layout\"",
        ],
        prompts: &[
            "the checkout button jumps on mobile, can you fix the layout?",
            "add a summary step before payment",
            "why does the cart total round wrong for 3 items?",
            "write tests for the new payment step",
            "make the error message friendlier when the card is declined",
        ],
    },
    Project {
        folder: "api",
        window: "🧩 API",
        files: &["src/auth/login.rs", "src/auth/session.rs", "src/routes/users.rs", "tests/login.rs", "Cargo.toml"],
        commands: &[
            "cargo test",
            "cargo clippy --all-targets",
            "cargo build",
            "git log --oneline -5",
            "git status",
            "git commit -m \"Fix flaky test\"",
        ],
        prompts: &[
            "the login test fails every few runs, find out why",
            "add rate limiting to the login route",
            "refresh tokens should expire after 30 days",
            "clean up the error types in auth",
        ],
    },
    Project {
        folder: "docs",
        window: "📚 Docs",
        files: &["guide/getting-started.md", "guide/configuration.md", "CHANGELOG.md", "release-notes/2.4.md"],
        commands: &[
            "npm run docs:build",
            "git status",
            "git diff",
            "npx markdownlint guide",
            "git commit -m \"Notes\"",
        ],
        prompts: &[
            "write the release notes for 2.4 from the merged PRs",
            "the configuration page is out of date, update it",
            "make the getting started guide shorter",
        ],
    },
    Project {
        folder: "infra",
        window: "🏗️ Infra",
        files: &[".github/workflows/ci.yml", "terraform/main.tf", "terraform/variables.tf", "docker/Dockerfile"],
        commands: &[
            "terraform plan",
            "terraform validate",
            "docker build -t app .",
            "git push",
            "gh run list --limit 3",
        ],
        prompts: &[
            "move the CI from Jenkins to GitHub Actions",
            "cache the dependencies in the pipeline",
            "why is the docker image 2 GB?",
        ],
    },
    Project {
        folder: "dashboard",
        window: "📊 Dashboard",
        files: &["src/theme.ts", "src/components/Chart.tsx", "src/components/Sidebar.tsx", "src/App.tsx"],
        commands: &["npm test", "npm run dev", "git status", "npm run build"],
        prompts: &[
            "add a dark mode that follows the system setting",
            "the chart legend overlaps on small screens",
            "use the new colors from the design file",
        ],
    },
    Project {
        folder: "payments",
        window: "💳 Payments",
        files: &["src/service.py", "src/providers/stripe.py", "tests/test_refunds.py", "pyproject.toml"],
        commands: &["pytest -q", "ruff check .", "git status", "python -m mypy src", "git commit -m \"Providers\""],
        prompts: &[
            "split the payment service into providers",
            "refunds over 30 days should go to manual review",
            "add type hints to the service",
        ],
    },
];

const ERRORS: &[&str] = &[
    "Exit code 1\nnpm ERR! Test failed. See above for more details.",
    "File has not been read yet. Read it first before writing to it.",
    "Exit code 101\nerror[E0308]: mismatched types",
    "String to replace not found in file.",
    "Exit code 1\nError: terraform plan failed: missing variable \"region\"",
];

/// A session to generate.
struct Spec {
    project: usize,
    title: &'static str,
    /// When its last activity was, before now.
    ended_ago: i64,
    turns: usize,
    /// Status in Claude Code's session list, if it is still open.
    open: Option<&'static str>,
    /// Ends with a subagent still running.
    agent_running: bool,
    /// Ends with a command still running in the background.
    background: bool,
    cache_1h: bool,
    /// Ends with its context nearly full.
    full_context: bool,
}

const SPECS: &[Spec] = &[
    Spec {
        project: 0,
        title: "Checkout flow redesign",
        ended_ago: 40,
        turns: 9,
        open: Some("busy"),
        agent_running: true,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 1,
        title: "Fix the flaky login test",
        ended_ago: 3 * MINUTE,
        turns: 6,
        open: Some("waiting"),
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 2,
        title: "Release notes for 2.4",
        ended_ago: 70,
        turns: 4,
        open: Some("busy"),
        agent_running: false,
        background: false,
        cache_1h: false,
        full_context: true,
    },
    Spec {
        project: 3,
        title: "Move CI to GitHub Actions",
        ended_ago: 52 * MINUTE,
        turns: 7,
        open: Some("busy"),
        agent_running: false,
        background: true,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 4,
        title: "Dark mode for the dashboard",
        ended_ago: 3 * HOUR,
        turns: 8,
        open: Some("idle"),
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 5,
        title: "Split the payment service",
        ended_ago: 26 * HOUR,
        turns: 12,
        open: Some("idle"),
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 0,
        title: "Faster product search",
        ended_ago: 2 * DAY,
        turns: 10,
        open: None,
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 1,
        title: "Rate limiting for login",
        ended_ago: 3 * DAY + 4 * HOUR,
        turns: 7,
        open: None,
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 3,
        title: "Smaller Docker image",
        ended_ago: 5 * DAY,
        turns: 6,
        open: None,
        agent_running: false,
        background: false,
        cache_1h: false,
        full_context: false,
    },
    Spec {
        project: 4,
        title: "Chart legend on small screens",
        ended_ago: 6 * DAY + 2 * HOUR,
        turns: 5,
        open: None,
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 5,
        title: "Manual review for old refunds",
        ended_ago: 9 * DAY,
        turns: 9,
        open: None,
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 2,
        title: "Update the configuration guide",
        ended_ago: 12 * DAY,
        turns: 6,
        open: None,
        agent_running: false,
        background: false,
        cache_1h: false,
        full_context: false,
    },
    Spec {
        project: 0,
        title: "Tests for the payment step",
        ended_ago: 16 * DAY,
        turns: 8,
        open: None,
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 1,
        title: "Refresh tokens expire after 30 days",
        ended_ago: 21 * DAY,
        turns: 7,
        open: None,
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
    Spec {
        project: 3,
        title: "Cache dependencies in CI",
        ended_ago: 27 * DAY,
        turns: 5,
        open: None,
        agent_running: false,
        background: false,
        cache_1h: true,
        full_context: false,
    },
];

fn session_id(i: usize) -> String {
    format!("0000demo-{i:04x}-4000-8000-{:012x}", 0x5eed_0000 + i)
}

/// Moves a time out of the night, as people mostly work in the day.
fn daytime(t: i64, utc_offset: i64) -> i64 {
    let hour = (t + utc_offset).rem_euclid(DAY) / HOUR;
    if (1..8).contains(&hour) { t - (hour + 1) * HOUR } else { t }
}

/// The transcript lines of one session, ending `spec.ended_ago` before `now`.
fn transcript(rng: &mut Rng, spec: &Spec, id: &str, cwd: &str, now: i64, utc_offset: i64) -> Vec<String> {
    let p = &PROJECTS[spec.project];
    // (seconds from the start, line) pairs, timed afterwards.
    let mut events: Vec<(i64, Value)> = Vec::new();
    let mut t = 0i64;
    // What Claude Code loads first: skills, MCP servers, agents, instructions, memory.
    for line in setup_lines(spec.project) {
        events.push((t, line));
    }
    let mut context = if spec.full_context { 760_000 } else { rng.range(38_000, 52_000) as u64 };
    let compact_at = if spec.full_context { 965_000 } else { 650_000 };
    let mut n = 0u32;
    let mut next_id = |prefix: &str| {
        n += 1;
        format!("{prefix}_demo{id}{n:05}", id = &id[9..13])
    };
    for turn in 0..spec.turns {
        let prompt = if turn == 0 { spec.title.to_string() } else { rng.pick(p.prompts).to_string() };
        events.push((t, json!({"type": "user", "message": {"role": "user", "content": prompt}})));
        let steps = rng.range(3, 12);
        for step in 0..steps {
            t += rng.range(8, 75);
            let mut content = Vec::new();
            let mut results = Vec::new();
            let last = turn + 1 == spec.turns && step + 1 == steps;
            if !last || spec.agent_running || spec.background || spec.open == Some("waiting") {
                let tool_id = next_id("toolu");
                let (name, input) = if last && spec.agent_running {
                    ("Agent", json!({"subagent_type": "Explore", "description": "Find where the totals are computed"}))
                } else if last && spec.background {
                    (
                        "Bash",
                        json!({"command": "gh run watch --exit-status", "description": "Wait for the CI pipeline", "run_in_background": true}),
                    )
                } else if last {
                    ("Bash", json!({"command": rng.pick(p.commands)}))
                } else {
                    match rng.range(0, 9) {
                        0..=3 => ("Bash", json!({"command": rng.pick(p.commands)})),
                        4 | 5 => {
                            ("Read", json!({"file_path": format!("{cwd}\\{}", rng.pick(p.files).replace('/', "\\"))}))
                        }
                        6 => {
                            let lines = rng.range(1, 30) as usize;
                            (
                                "Edit",
                                json!({"file_path": format!("{cwd}\\{}", rng.pick(p.files).replace('/', "\\")),
                                       "old_string": "x\n".repeat(rng.range(1, 12) as usize),
                                       "new_string": "y\n".repeat(lines)}),
                            )
                        }
                        7 => (
                            "Write",
                            json!({"file_path": format!("{cwd}\\{}", rng.pick(p.files).replace('/', "\\")),
                                              "content": "line\n".repeat(rng.range(10, 120) as usize)}),
                        ),
                        8 => ("Grep", json!({"pattern": "TODO|FIXME", "path": "src"})),
                        _ => ("WebFetch", json!({"url": "https://docs.example.com/guide", "prompt": "summarise"})),
                    }
                };
                content.push(json!({"type": "tool_use", "id": tool_id, "name": name, "input": input}));
                if last && spec.background {
                    // And an agent in the background, still working after the turn.
                    let agent_id = next_id("toolu");
                    let input = json!({"subagent_type": "general-purpose", "description": "Review the deploy job", "run_in_background": true});
                    content.push(json!({"type": "tool_use", "id": agent_id, "name": "Agent", "input": input}));
                    let text = "Async agent launched successfully.\nagentId: a1demo (use it to send messages)";
                    results.push(json!({"type": "tool_result", "tool_use_id": agent_id, "content": text}));
                }
                let pending = last && (spec.agent_running || spec.open == Some("waiting"));
                if !pending {
                    let (text, error) = if last && spec.background {
                        ("Command running in background with ID: bdemo1. Output is being written to: tasks\\bdemo1.output".to_string(), false)
                    } else if rng.chance(7) {
                        (rng.pick(ERRORS).to_string(), true)
                    } else {
                        ("ok".to_string(), false)
                    };
                    results.push(
                        json!({"type": "tool_result", "tool_use_id": tool_id, "content": text, "is_error": error}),
                    );
                }
            } else {
                content.push(json!({"type": "text", "text": "Done."}));
            }
            // A nearly full one grows slowly, so it ends between about 770k and 900k.
            context += if spec.full_context { rng.range(1_000, 3_000) } else { rng.range(1_500, 9_000) } as u64;
            if context > compact_at {
                events.push((t, json!({"type": "system", "subtype": "compact_boundary"})));
                context = rng.range(45_000, 70_000) as u64;
            }
            let write = rng.range(500, 6_000) as u64;
            let output = rng.range(150, 2_800) as u64;
            let (one_hour, five_min) = if spec.cache_1h { (write, 0) } else { (0, write) };
            let mut line = json!({
                "type": "assistant",
                "message": {
                    "id": next_id("msg"),
                    "model": if spec.project == 2 { "claude-sonnet-5-5" } else { "claude-opus-5-5" },
                    "usage": {
                        "input_tokens": rng.range(1, 40),
                        "cache_read_input_tokens": context,
                        "cache_creation_input_tokens": write,
                        "cache_creation": {"ephemeral_1h_input_tokens": one_hour, "ephemeral_5m_input_tokens": five_min},
                        "output_tokens": output,
                        "output_tokens_details": {"thinking_tokens": output / 3}
                    },
                    "content": content
                }
            });
            if spec.cache_1h {
                line["promptCacheRequested"] = json!("1h");
            }
            events.push((t, line));
            for r in results {
                t += rng.range(1, 20);
                events.push((t, json!({"type": "user", "message": {"role": "user", "content": [r]}})));
            }
        }
        if turn + 1 < spec.turns {
            // A pause before the next prompt: minutes, sometimes hours.
            t += if rng.chance(20) { rng.range(2 * HOUR, 20 * HOUR) } else { rng.range(2 * MINUTE, 40 * MINUTE) };
        }
    }
    events.push((t, json!({"type": "custom-title", "customTitle": spec.title})));
    // Time it so the last activity is `ended_ago` before now.
    let end = now - spec.ended_ago;
    let start = if spec.open.is_some() { end - t } else { daytime(end, utc_offset) - t };
    events
        .into_iter()
        .map(|(offset, mut v)| {
            v["timestamp"] = json!(rfc3339(start + offset));
            v["sessionId"] = json!(id);
            v["cwd"] = json!(cwd);
            v.to_string()
        })
        .collect()
}

/// The start-up entries of a made-up session (see `setup.rs`); a little
/// different per project.
fn setup_lines(project: usize) -> Vec<Value> {
    let text = |n: usize| "Made-up demo text. ".repeat(n / 19 + 1);
    let skills: Vec<String> = (0..14 + project * 2).map(|i| format!("demo-skill-{i}")).collect();
    let mut tools: Vec<String> = ["search", "get_issue", "create_issue", "comment", "transition", "log_work"]
        .iter()
        .map(|t| format!("mcp__tracker__{t}"))
        .collect();
    tools.extend(["query", "read"].iter().map(|t| format!("mcp__docs__{t}")));
    if project % 2 == 0 {
        tools.extend(["scan", "issues", "hotspots"].iter().map(|t| format!("mcp__quality__{t}")));
    }
    tools.extend(["WebFetch", "WebSearch", "NotebookEdit"].iter().map(|t| t.to_string()));
    let lines: Vec<String> = tools.iter().map(|t| format!("{t}: {}", text(60))).collect();
    let agents = ["general-purpose", "Explore", "Plan", "code-reviewer", "docs-writer"];
    let mut files = vec![
        json!({"path": "~/.claude/CLAUDE.md", "type": "User", "content": text(2_400)}),
        json!({"path": "CLAUDE.md", "type": "Project", "content": text(3_000 + project * 1_500)}),
    ];
    if project % 3 == 0 {
        files.push(json!({"path": "memory/MEMORY.md", "type": "AutoMem", "content": text(700)}));
    }
    let attachment = |a: Value| json!({"type": "attachment", "attachment": a});
    vec![
        attachment(json!({"type": "prompt_snapshot", "systemPrompt": [text(9_000), text(3_500)]})),
        attachment(
            json!({"type": "skill_listing", "content": text(14_000 + project * 900), "skillCount": skills.len(), "isInitial": true, "names": skills}),
        ),
        attachment(
            json!({"type": "deferred_tools_delta", "addedNames": tools, "addedLines": lines, "removedNames": []}),
        ),
        attachment(
            json!({"type": "mcp_instructions_delta", "addedNames": ["docs"], "addedBlocks": [text(3_800)], "removedNames": []}),
        ),
        attachment(
            json!({"type": "agent_listing_delta", "addedTypes": agents, "addedLines": agents.map(|a| format!("{a}: {}", text(110))), "isInitial": true}),
        ),
        attachment(json!({"type": "instructions", "files": files})),
    ]
}

fn write(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

/// The entry for an open session in Claude Code's session list.
fn session_entry(i: usize, id: &str, cwd: &str, status: &str, since_ms: i64) -> String {
    json!({
        "pid": PID_BASE + i as u32,
        "sessionId": id,
        "cwd": cwd,
        "startedAt": since_ms - 6 * HOUR * 1000,
        "kind": "interactive",
        "entrypoint": "claude-vscode",
        "status": status,
        "statusUpdatedAt": since_ms,
    })
    .to_string()
}

/// Claude Code's project folder name for a working directory.
fn project_folder(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Writes the demo folder and switches the app to it. Call before anything
/// reads Claude Code's folder.
pub fn start() {
    let base = std::env::temp_dir().join("trayward-demo");
    let _ = std::fs::remove_dir_all(&base);
    let config = base.join(".claude");
    let now = crate::win::now_ms() / 1000;
    let utc_offset = crate::timefmt::utc_offset(now);
    let mut rng = Rng(0x007e_a5ad);
    for (i, spec) in SPECS.iter().enumerate() {
        let p = &PROJECTS[spec.project];
        let workspace = base.join("projects").join(p.folder);
        write(
            &workspace.join(".vscode").join("settings.json"),
            &json!({"window.title": format!("{} | ${{activeEditorShort}}", p.window)}).to_string(),
        );
        let cwd = workspace.display().to_string();
        let id = session_id(i);
        let lines = transcript(&mut rng, spec, &id, &cwd, now, utc_offset);
        write(
            &config.join("projects").join(project_folder(&cwd)).join(format!("{id}.jsonl")),
            &(lines.join("\n") + "\n"),
        );
        if let Some(status) = spec.open {
            let since_ms = (now - spec.ended_ago) * 1000;
            let entry = session_entry(i, &id, &cwd, status, since_ms);
            write(&config.join("sessions").join(format!("{}.json", PID_BASE + i as u32)), &entry);
            // The VS Code window that has the project open, as Claude Code's extension records it.
            let lock = json!({"pid": PID_BASE + 100 + spec.project as u32, "workspaceFolders": [cwd], "ideName": "Visual Studio Code"});
            write(&config.join("ide").join(format!("{}.lock", 40_000 + spec.project)), &lock.to_string());
        }
    }
    // Subagents of the first session, for its agent map: two finished, one still running.
    let cwd = base.join("projects").join(PROJECTS[SPECS[0].project].folder).display().to_string();
    let dir = config.join("projects").join(project_folder(&cwd)).join(session_id(0)).join("subagents");
    for (n, (description, kind, minutes, running)) in [
        ("Find where the totals are computed", "Explore", 4, false),
        ("Review the checkout styles", "fork", 13, false),
        ("Write the payment form tests", "fork", 21, true),
    ]
    .into_iter()
    .enumerate()
    {
        let start = now - 30 * 60 + n as i64 * 4 * 60;
        let lines: Vec<String> = (0..minutes)
            .map(|m| {
                json!({
                    "type": "assistant", "isSidechain": true, "timestamp": rfc3339(start + m * 60),
                    "message": {"id": format!("msg_demo_agent{n}_{m}"), "model": "claude-opus-5-5",
                        "usage": {"input_tokens": 3, "cache_read_input_tokens": 30_000 + m * 9_000, "cache_creation_input_tokens": 2_000, "output_tokens": 900},
                        "content": [{"type": "tool_use", "id": format!("toolu_demo_agent{n}_{m}"), "name": "Read", "input": {"file_path": "src/cart.ts"}}]}
                })
                .to_string()
            })
            .collect();
        write(
            &dir.join(format!("agent-demo{n}.jsonl")),
            &(lines.join(
                "
",
            ) + "
"),
        );
        let mut meta = json!({"agentType": kind, "description": description, "spawnDepth": 1});
        if !running {
            meta["finished"] = json!(true);
        }
        write(&dir.join(format!("agent-demo{n}.meta.json")), &meta.to_string());
    }
    // Memory files for two projects.
    for project in [0, 3] {
        let cwd = base.join("projects").join(PROJECTS[project].folder).display().to_string();
        let memory = config.join("projects").join(project_folder(&cwd)).join("memory");
        for (name, text) in
            [("MEMORY.md", "- [Build](build.md)"), ("build.md", "Run the tests first."), ("style.md", "Short names.")]
        {
            write(&memory.join(name), text);
        }
    }
    // A little file history, so the storage page has something to show.
    for i in 0..4 {
        write(&config.join("file-history").join(session_id(i)).join("v1"), &"x".repeat(40_000 * (i + 1)));
    }
    codex(&base.join(".codex"), now);
    copilot(&base.join(".copilot"), now);
    mailbox(&base.join(".claude-mailbox"), now);
    let _ = STARTED.set(std::time::Instant::now());
    let _ = DIR.set(config.clone());
    std::thread::spawn(move || direct(&config));
}

/// When the demo started.
static STARTED: OnceLock<std::time::Instant> = OnceLock::new();

/// Made-up merge requests for the demo's projects (no GitLab or GitHub is
/// asked): one pipeline running, one failed, one passed; the running one
/// passes about a minute after the demo started.
pub fn forge(folders: &[String]) -> crate::forge::Snapshot {
    use crate::forge::{Branch, Change, Checks, Forge, Snapshot, State};
    let passed_by_now = STARTED.get().is_some_and(|t| t.elapsed().as_secs() > 70);
    // And a pull request on GitHub is merged a minute later (forge is asked every minute).
    let merged_by_now = STARTED.get().is_some_and(|t| t.elapsed().as_secs() > 125);
    let mr = |number: u64, project: &str, title: &str, checks: Option<Checks>, draft: bool| Change {
        forge: Forge::GitLab,
        number,
        title: title.into(),
        url: format!("https://gitlab.example.com/acme/{project}/-/merge_requests/{number}"),
        project: format!("acme/{project}"),
        state: State::Open,
        draft,
        checks,
        updated: 0,
    };
    let shop = mr(
        42,
        "shop",
        "Checkout flow redesign",
        Some(if passed_by_now { Checks::Passed } else { Checks::Running }),
        false,
    );
    let api = mr(39, "api", "Fix the flaky login test", Some(Checks::Failed), false);
    let infra = mr(17, "infra", "Move CI to GitHub Actions", Some(Checks::Passed), false);
    let mut snap = Snapshot { sessions: Default::default(), mine: vec![shop.clone(), api.clone(), infra.clone()] };
    snap.mine.push(Change {
        forge: Forge::GitHub,
        number: 12,
        title: "Add a dark mode".into(),
        url: "https://github.com/acme/dashboard/pull/12".into(),
        project: "acme/dashboard".into(),
        state: if merged_by_now { State::Merged } else { State::Open },
        draft: !merged_by_now,
        checks: None,
        updated: 0,
    });
    for folder in folders {
        let project = Path::new(folder).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let branch = |name: &str, change: Option<&Change>, checks| Branch {
            name: name.into(),
            change: change.cloned(),
            checks: change.and_then(|c| c.checks).or(checks),
        };
        let b = match project.as_str() {
            "shop" => branch("feat/checkout-redesign", Some(&shop), None),
            "api" => branch("fix/flaky-login", Some(&api), None),
            "infra" => branch("ci/github-actions", Some(&infra), None),
            "docs" => branch("main", None, Some(Checks::Passed)),
            _ => continue,
        };
        snap.sessions.insert(folder.clone(), b);
    }
    // Like the real one: only requests in a project a session works in.
    let names: Vec<String> = folders
        .iter()
        .filter_map(|f| Path::new(f).file_name().map(|n| format!("acme/{}", n.to_string_lossy())))
        .collect();
    let projects: Vec<&str> = names.iter().map(String::as_str).collect();
    snap.mine.retain(|c| crate::forge::in_projects(c, &projects));
    snap
}

/// The demo's Codex folder (next to its `.claude`).
pub fn codex_dir() -> Option<PathBuf> {
    Some(dir()?.parent()?.join(".codex"))
}

/// The demo's session mailbox (next to its `.claude`).
pub fn mailbox_dir() -> Option<PathBuf> {
    Some(dir()?.parent()?.join(".claude-mailbox"))
}

/// A session mailbox: two messages waiting for the shop's session, one for
/// the API's, and some handled.
fn mailbox(root: &Path, now: i64) {
    let at = |t: i64| crate::timefmt::rfc3339(t);
    let stamp = |t: i64| {
        let s: String = at(t).chars().filter(char::is_ascii_alphanumeric).collect();
        // 20261114T090000Z → 20261114T0900Z
        format!("{}Z", &s[..13])
    };
    let message = |inbox: &str, done: bool, from: &str, topic: &str, ago: i64| {
        let t = now - ago;
        let folder = if done { root.join(inbox).join("done") } else { root.join(inbox) };
        let text = format!("From: {from} (wsl)\nTo: {inbox}\nDate: {}\n\nA made-up message.\n", at(t));
        let path = folder.join(format!("{}-{from}-{topic}.md", stamp(t)));
        write(&path, &text);
        // "Last handled" reads a handled file's time: an hour after it came.
        if done && let Ok(file) = std::fs::File::options().write(true).open(&path) {
            let _ = file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs((t + HOUR) as u64));
        }
    };
    write(&root.join("README.md"), "# The demo's mailbox\n");
    message("shop", false, "infra", "staging-database-moved", 25 * MINUTE);
    message("shop", false, "api", "new-order-endpoint", 6 * MINUTE);
    message("api", false, "infra", "certificate-renewal", 2 * HOUR);
    message("api", true, "shop", "cart-totals-question", DAY);
    message("docs", true, "api", "changelog-for-v2", 2 * DAY);
    message("infra", true, "shop", "ports-for-the-preview", 3 * DAY);
}

/// The demo's Copilot folder (next to its `.claude`).
pub fn copilot_dir() -> Option<PathBuf> {
    Some(dir()?.parent()?.join(".copilot"))
}

/// A Copilot session that asks the user something.
fn copilot(home: &Path, now: i64) {
    let id = "c0b1107a-0000-4000-8000-00000000c0b1";
    let start = now - 6 * MINUTE;
    let cwd = std::env::temp_dir().join("trayward-demo").join("projects").join("docs").display().to_string();
    let event = |kind: &str, data: Value, t: i64| json!({"type": kind, "data": data, "id": "demo", "timestamp": crate::timefmt::rfc3339(t), "parentId": null});
    let lines = [
        event("session.start", json!({"sessionId": id, "producer": "copilot-agent", "context": {"cwd": cwd}}), start),
        event("user.message", json!({"content": "Update the install guide for the new CLI"}), start + 5),
        event("assistant.turn_start", json!({"turnId": "0"}), start + 5),
        event(
            "session.usage_record",
            json!({"usage": {"model": "gpt-5.5", "modelDisplayName": "GPT-5.5", "inputTokens": 31_125, "cost": 1.0}}),
            start + 20,
        ),
        event("tool.execution_start", json!({"toolCallId": "call_1", "toolName": "ask_user"}), now - 2 * MINUTE),
    ];
    let dir = home.join("session-state").join(id);
    let text: Vec<String> = lines.iter().map(Value::to_string).collect();
    write(&dir.join("events.jsonl"), &(text.join("\n") + "\n"));
    write(&dir.join("workspace.yaml"), &format!("cwd: {cwd}\nid: {id}\nname: Update the install guide\n"));
    // A process of the demo holds it (see `win::process_alive`).
    write(&dir.join(format!("inuse.{}.lock", PID_BASE + 900)), &(PID_BASE + 900).to_string());
}

/// Copilot's monthly limit on a Pro plan.
pub fn copilot_quota(now: i64) -> crate::copilot::Quota {
    crate::copilot::Quota {
        kind: crate::copilot::Kind::Premium,
        limit: crate::usage::Limit { used: 25.0, resets_at: Some(now + 17 * DAY) },
        remaining: 225.0,
        entitlement: 300.0,
        plan: Some("Pro".into()),
        at: now,
    }
}

/// Two Codex sessions: one done a few minutes ago, one waiting for an
/// approval; with the limits of a ChatGPT Plus plan.
fn codex(home: &Path, now: i64) {
    let limits = json!({
        "primary": {"used_percent": 18.0, "window_minutes": 300, "resets_at": now + 3 * HOUR + 20 * MINUTE},
        "secondary": {"used_percent": 41.0, "window_minutes": 10080, "resets_at": now + 4 * DAY + 2 * HOUR},
        "plan_type": "plus",
    });
    let sessions = [
        ("0199c0de-0000-7000-8000-00000000c0d1", "payments", "Add retries to the payment webhook", 3 * MINUTE, true),
        ("0199c0de-0000-7000-8000-00000000c0d2", "api", "Run the new database migration", MINUTE, false),
    ];
    for (id, project, prompt, ago, done) in sessions {
        let end = now - ago;
        let start = end - 4 * MINUTE;
        let cwd = std::env::temp_dir().join("trayward-demo").join("projects").join(project).display().to_string();
        let at = |t: i64| crate::timefmt::rfc3339(t);
        let mut lines = vec![
            json!({"timestamp": at(start), "type": "session_meta", "payload": {"id": id, "timestamp": at(start), "cwd": cwd, "originator": "codex_cli_rs", "cli_version": "0.63.0"}}),
            json!({"timestamp": at(start), "type": "turn_context", "payload": {"cwd": cwd, "model": "gpt-5.5-codex"}}),
            json!({"timestamp": at(start + 2), "type": "event_msg", "payload": {"type": "user_message", "message": prompt}}),
            json!({"timestamp": at(end - 30), "type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 48_210}}, "rate_limits": limits}}),
        ];
        lines.push(if done {
            json!({"timestamp": at(end), "type": "event_msg", "payload": {"type": "agent_message", "message": "Done: retries with backoff, and a test."}})
        } else {
            json!({"timestamp": at(end), "type": "response_item", "payload": {"type": "function_call", "name": "shell", "arguments": "{\"command\":[\"npm\",\"run\",\"migrate\"]}", "call_id": "call_1"}})
        });
        let day = at(start);
        let (y, m, d) = (&day[0..4], &day[5..7], &day[8..10]);
        let path = home
            .join("sessions")
            .join(y)
            .join(m)
            .join(d)
            .join(format!("rollout-{}-{id}.jsonl", day[..19].replace(':', "-")));
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        write(&path, &(text.join("\n") + "\n"));
        // The status comes from when the file last changed.
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(end as u64));
        }
    }
}

/// Moves sessions along like real work: two finish shortly after start,
/// after the quiet start-up seconds, so they announce themselves (bubble, sound).
fn direct(config: &Path) {
    std::thread::sleep(std::time::Duration::from_secs(12));
    let now_ms = crate::win::now_ms();
    for (i, spec) in SPECS.iter().enumerate() {
        if spec.open == Some("busy") && !spec.agent_running {
            let cwd = config.parent().unwrap_or(config).join("projects").join(PROJECTS[spec.project].folder);
            write(
                &config.join("sessions").join(format!("{}.json", PID_BASE + i as u32)),
                &session_entry(i, &session_id(i), &cwd.display().to_string(), "idle", now_ms),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcripts_read_back() {
        let now = 1_794_406_200;
        let mut rng = Rng(1);
        let lines = transcript(&mut rng, &SPECS[3], &session_id(3), "C:\\demo\\infra", now, 0);
        let mut stats = crate::stats::Stats::default();
        for l in &lines {
            stats.line(l);
        }
        assert_eq!(stats.title.as_deref(), Some("Move CI to GitHub Actions"));
        assert_eq!(stats.prompts.len(), SPECS[3].turns);
        assert!(stats.replies.len() > SPECS[3].turns);
        // The last line is a tool result, a few seconds after the last reply.
        let last = stats.last_at.unwrap();
        assert!((now - SPECS[3].ended_ago - last).abs() < 60);
        let h = history(now);
        assert!(h.iter().any(|s| s.used == WEEKLY_USED));
    }
}
