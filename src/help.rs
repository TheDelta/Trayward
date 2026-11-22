//! Help for the settings: the "?" after a card's title shows a short text on
//! hover and opens the card's section of the help page (`docs/settings.md`,
//! on GitHub) on click. The section is found by the card's title: its `###`
//! heading, as GitHub turns it into an anchor.

/// The help page, in the repository.
pub const PAGE: &str = "docs/settings.md";

/// Card title and its short help (English; the German is in `i18n/de.rs`, with all others).
pub const TOPICS: &[(&str, &str)] = &[
    // BEGIN TOPICS (generated from the help texts; keep the order of docs/settings.md)
    (
        "Start with Windows",
        "Trayward starts when you sign in. If you move the exe, start it once from its new place and the entry follows.",
    ),
    (
        "Refresh usage every",
        "How often Trayward asks Anthropic for your limits; when the service says to wait, it waits longer. ↻ in the flyout updates at once.",
    ),
    (
        "Animations",
        "Off: no glow or shake for sessions that need you, and no spinner. Colours and counts still tell you the same.",
    ),
    (
        "GPG key timer",
        "A thin line under the widget shows how long your GPG key stays unlocked. Only when git signs commits (commit.gpgsign); the card says what's missing.",
    ),
    (
        "Simulate a 5-hour limit",
        "Five-hour windows, each allowed a seventh of the weekly limit. Changes nothing on plans with a real 5-hour limit.",
    ),
    (
        "Language",
        "System: German if Windows' display language is German, English otherwise. Changes at once, no restart.",
    ),
    (
        "Position",
        "Widgets: over Windows' Widgets button, where app buttons never run under it. Needs Widgets on in Windows' taskbar settings.",
    ),
    (
        "Display mode",
        "Slim makes the widget narrower, for when it covers taskbar buttons. Also in the right-click menu.",
    ),
    (
        "Take turns",
        "Seconds per limit. Slim: all limits take turns. Off: Normal keeps the 5-hour limit, Slim the weekly one.",
    ),
    (
        "Usage bars",
        "The small usage bars in the widget. Never makes it narrower; Always shows them even when taskbar buttons run underneath.",
    ),
    (
        "Solid background",
        "Taskbar buttons under the widget are hidden instead of showing through. Rarely needed when it sits on the Widgets button.",
    ),
    (
        "Show on taskbars",
        "All includes displays connected later. Windows needs “Show my taskbar on all displays” on (Taskbar behaviors).",
    ),
    (
        "System monitor",
        "CPU, memory, GPU, network and disk next to the limits. Read once a second, only while on; nothing leaves the PC.",
    ),
    ("CPU", "How busy the processor was over the last second, in percent. The tooltip shows it too."),
    (
        "Memory (RAM)",
        "Physical memory in use, counted as Task Manager does. The tooltip shows used, size and percentage.",
    ),
    (
        "GPU",
        "%: the busiest engine, as Task Manager shows it. GB: video memory in use on the card with the most memory.",
    ),
    ("Network", "Real adapters only: the virtual ones of WSL and Hyper-V are left out, so nothing counts twice."),
    (
        "Disk",
        "%: busy time, like “active time” in Task Manager. Read/write: bytes per second, in a column of their own.",
    ),
    ("Graphs", "A faint graph behind each value. Off: just the numbers."),
    (
        "Instant updates",
        "Adds Trayward's hooks to Claude Code's settings.json; off removes exactly those. Open sessions may need a restart.",
    ),
    (
        "Session details",
        "Reads the sessions' transcripts. Off: no titles, cache timers or background work (👀) in the list.",
    ),
    (
        "Sessions in WSL",
        "Claude Code's sessions in WSL, in the list and the statistics. A stopped distribution is never started for it, and an unused one is left to stop.",
    ),
    (
        "Keep statistics",
        "Keeps what the statistics show: numbers, each prompt's start, tool calls with commands, files and errors. Not Claude's replies. The statistics open faster with it. Off deletes what was kept.",
    ),
    ("Hide archived sessions", "Archived in Claude Code's VS Code extension. Off: they're always listed, faded."),
    (
        "Check sessions every",
        "How often Claude Code's session list is read. With Instant updates on, most changes show at once anyway.",
    ),
    (
        "Bubble",
        "The switch for all bubbles: limit warnings, pipelines and updates too. None while you present or Do not disturb is on.",
    ),
    (
        "Sound when a session needs you",
        "Custom plays a WAV file of yours. It also plays when a prompt cache is about to run out or a pipeline fails.",
    ),
    (
        "Sound when a session is done",
        "Custom plays a WAV file of yours. It also plays when a limit is free again, a pipeline passes or a request is merged.",
    ),
    (
        "Volume",
        "A custom WAV plays as loud as it was recorded. Mute sounds in the right-click menu silences them all for now.",
    ),
    ("Weekly limit", "Each warning comes once per week. Copilot's monthly limit, when shown, warns at the same level."),
    (
        "5-hour limit",
        "Once per 5-hour window. It waits while you present or Do not disturb is on, and comes afterwards.",
    ),
    (
        "Daily budget",
        "Each day's share of the week: the days in the flyout's weekly bar and today's limit (D). Days run from midnight. Even: a seventh every day, by its hours.",
    ),
    (
        "Weekly bar",
        "Days: how much of its budget each day used. Budgets: the bar is the week, each day as wide as its budget.",
    ),
    ("Pace", "Goes by this week's average use per day; in the week's first hours, by your past weeks."),
    (
        "Countdown",
        "From that level on, the widget shows how long until the limit resets. Full: only once it's used up.",
    ),
    ("Free again", "For the weekly and the 5-hour limit, with the “done” sound."),
    (
        "Weekly summary",
        "From 08:00 on Monday: prompts, tokens, API value and how long Claude worked. A click opens the statistics' list of weeks.",
    ),
    (
        "Claude Code",
        "Read with the login Claude Code saved: usage from Anthropic, sessions from your PC. Without ~/.claude nothing is asked.",
    ),
    (
        "Fable limit",
        "Fable's own limit, a share of the weekly one, under Weekly in the flyout and in the tooltip. On by default; only on plans that have one.",
    ),
    ("Codex", "Read from Codex's own files: no sign-in, no network. Its limits are as of its last use."),
    (
        "In the taskbar",
        "All: Claude's and Codex's limits, and Copilot's monthly one when shown. Claude or Codex: only theirs.",
    ),
    (
        "Copilot",
        "With limit asks GitHub every 5 minutes through gh, with its login: install the GitHub CLI and run gh auth login.",
    ),
    (
        "Merge and pull requests",
        "Each session's MR or PR and its pipeline, through glab and gh: with Trayward's own token (GitLab and GitHub page), else their login.",
    ),
    (
        "glab",
        "GitLab's command-line tool: Trayward runs it for merge requests, with its own token for the host or glab's login.",
    ),
    (
        "gh",
        "GitHub's command-line tool: Trayward runs it for pull requests (its own token or gh's login) and Copilot's limit (gh's login).",
    ),
    (
        "GitLab token",
        "A read-only fine-grained GitLab token: Merge request, Pipeline and Project: Read; User: Read. Copy it, then Paste token. Click for how to make one.",
    ),
    (
        "GitHub token",
        "A read-only fine-grained GitHub token: Pull requests, Commit statuses and Actions: Read. Copy it, then Paste token. Click for how to make one.",
    ),
    ("Pipeline bubbles", "With the logo, number and title. Needs Bubble on (Notifications)."),
    (
        "Where it shows",
        "In the flyout (each session's MR or PR, your own open ones), as bubbles when a pipeline ends or a request is merged, or both.",
    ),
    ("Away mode", "Pushes go to your push service. Only the session's name and state go out, never prompts or paths."),
    ("Away after", "A locked PC counts as away the same way. Snoozed notifications stop the pushes too."),
    ("Done too", "Off: only sessions that need you are pushed. On: finished ones too."),
    (
        "Push service",
        "Edit: pick ntfy, Gotify, Discord, Slack or a webhook and fill in what it needs, or paste an address there. Kept encrypted; Test sends one, Remove forgets it.",
    ),
    (
        "MQTT",
        "For Home Assistant and other MQTT tools: a device with sensors for limits and sessions, and events for automations.",
    ),
    (
        "Broker",
        "Edit: host, port, user name and password, as Home Assistant shows them, or paste a whole mqtt:// address there. The password is kept encrypted; Remove forgets it all.",
    ),
    (
        "Mailbox",
        "Unread and handled mail in the flyout, and ✉ on a session with mail waiting. Trayward only reads the folder.",
    ),
    (
        "Folder",
        "Looks for .claude-mailbox above your sessions' folders. Elsewhere? In Explorer: Copy as path, then Paste.",
    ),
    (
        "Your custom extensions",
        "Your own programs write JSON files that Trayward only reads: rows for the tooltip and the flyout. Nothing in them is run, opened or sent.",
    ),
    (
        "Extensions folder",
        "Where your programs put their files, one per extension. Each one found gets its switch below, off until you switch it on.",
    ),
    ("Check for updates", "It only tells you: nothing is downloaded or installed. Check now asks right away."),
    (
        "Location",
        "The folder trayward.exe is in. To replace it with a newer one, exit Trayward first. Moved? Start it once from there: Start with Windows and the hooks follow.",
    ),
    // END TOPICS
];

/// Each settings page and what's on it, as the help page's list of pages says
/// (there with a small first letter). The "?" after a page's title shows it and
/// leads to the page's section. About has none: its cards have their own "?".
pub const PAGES: &[(&str, &str)] = &[
    ("General", "Starting with Windows, how often usage is read, animations, language"),
    ("Taskbar", "Where the widget sits and how it looks"),
    ("Sessions", "How sessions are found and what's read about them"),
    ("Notifications", "Bubbles and sounds"),
    ("Limits", "Warnings, the daily budget, the weekly bar and the countdown"),
    ("Providers", "The agents Trayward reads: Claude Code, Codex, Copilot"),
    ("Extensions", "Optional features: each adds its settings while it's on"),
    ("System monitor", "Your PC's load next to the limits"),
    ("GitLab and GitHub", "glab and gh, and Trayward's own tokens for them"),
    ("Away", "Notifications on your phone while you're away"),
    ("MQTT", "Limits and sessions for Home Assistant"),
    ("Mailbox", "Mail between sessions in a shared folder"),
    ("Custom extensions", "What your own programs report"),
];

/// What's on the settings page named `page`, if it has help.
pub fn page_tip(page: &str) -> Option<&'static str> {
    PAGES.iter().find(|(p, _)| *p == page).map(|(_, tip)| *tip)
}

/// Where the help of the settings page named `page` starts.
pub fn page_url(page: &str) -> String {
    url(&format!("{page} page"))
}

/// The short help of the card titled `title`, if it has any.
pub fn tip(title: &str) -> Option<&'static str> {
    TOPICS.iter().find(|(t, _)| *t == title).map(|(_, tip)| *tip)
}

/// Where the help of the card titled `title` is: its section of the help page.
pub fn url(title: &str) -> String {
    format!("{}/blob/main/{PAGE}#{}", crate::update::REPO_URL, anchor(title))
}

/// A heading's anchor as GitHub makes it: lower case, spaces as hyphens,
/// letters, digits, hyphens and underscores kept, everything else dropped.
pub fn anchor(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = include_str!("../docs/settings.md");

    fn headings() -> Vec<&'static str> {
        DOC.lines().filter_map(|l| l.strip_prefix("### ")).map(str::trim).collect()
    }

    #[test]
    fn anchors_like_github() {
        assert_eq!(anchor("Memory (RAM)"), "memory-ram");
        assert_eq!(anchor("5-hour limit"), "5-hour-limit");
        assert_eq!(anchor("Start with Windows"), "start-with-windows");
        assert!(url("Usage bars").ends_with("/blob/main/docs/settings.md#usage-bars"));
    }

    /// The core names the project itself (its `Cargo.toml` has no `repository`):
    /// the links here, the update check and MQTT's discovery all go by it.
    #[test]
    fn links_go_to_the_project_in_cargo_toml() {
        assert_eq!(crate::update::REPO_URL, env!("CARGO_PKG_REPOSITORY"));
        assert_eq!(url("Usage bars"), "https://github.com/TheDelta/Trayward/blob/main/docs/settings.md#usage-bars");
        assert_ne!(crate::app::VERSION, "0.0.0");
    }

    #[test]
    fn every_topic_has_its_section_and_every_section_its_topic() {
        let found = headings();
        for (title, tip) in TOPICS {
            assert!(found.contains(title), "docs/settings.md has no '### {title}'");
            assert!(!tip.is_empty() && tip.len() <= 200, "the help of {title} is empty or too long");
        }
        for h in &found {
            assert!(tip(h).is_some(), "'### {h}' in docs/settings.md has no help in the app");
        }
        // One heading per anchor, of any level: GitHub numbers repeated ones
        // ("mqtt-1"), and the link would land on the first.
        let all: Vec<String> =
            DOC.lines().filter(|l| l.starts_with('#')).map(|l| anchor(l.trim_start_matches('#').trim())).collect();
        let mut unique = all.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), all.len(), "two headings of docs/settings.md share an anchor");
    }

    #[test]
    fn every_page_but_about_has_its_section_and_its_line_in_the_list() {
        for (name, _) in crate::settings_window::PAGES {
            let heading = format!("## {name} page");
            assert!(DOC.lines().any(|l| l == heading), "docs/settings.md has no '{heading}'");
            assert_eq!(page_tip(name).is_some(), name != "About", "{name}: help for every page but About");
        }
        for (name, tip) in PAGES {
            let mut first = tip.chars();
            let lower = first.next().map(|c| c.to_lowercase().chain(first).collect::<String>()).unwrap();
            // "MQTT" and the like keep their capitals.
            let line = format!("- [{name}](#{}): ", anchor(&format!("{name} page")));
            let found = DOC.lines().find(|l| l.starts_with(&line)).unwrap_or_else(|| panic!("no '{line}'"));
            let said = found.strip_prefix(&line).unwrap_or_default();
            assert!(said == lower || said == *tip, "docs/settings.md says '{said}' for {name}, the app '{tip}'");
        }
        assert!(page_url("System monitor").ends_with("#system-monitor-page"));
    }

    #[test]
    fn every_tip_is_translated() {
        use crate::i18n::{Choice, set, tr};
        set(Choice::German);
        for (title, en) in TOPICS.iter().chain(PAGES) {
            let de = tr(en);
            assert!(de != *en, "no German for {title}: {en}");
            assert!(de.len() <= 220, "the German help of {title} is too long");
        }
        set(Choice::English);
    }
}
