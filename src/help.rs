//! Help for the settings: the "?" after a card's title shows a short text on
//! hover and opens the card's section of the help page (`docs/settings.md`,
//! on GitHub) on click. The section is found by the card's title: its `###`
//! heading, as GitHub turns it into an anchor.

/// The help page, in the repository.
pub const PAGE: &str = "docs/settings.md";

/// Card title and its short help (English; the German is in [`DE`]).
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
/// leads to the page's section. About has none: its one setting has its own "?".
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

/// German translations of the short help.
pub const DE: &[(&str, &str)] = &[
    // BEGIN DE
    (
        "Starting with Windows, how often usage is read, animations, language",
        "Mit Windows starten, wie oft die Nutzung gelesen wird, Animationen, Sprache",
    ),
    ("Where the widget sits and how it looks", "Wo das Widget sitzt und wie es aussieht"),
    ("Your PC's load next to the limits", "Die Last deines PCs neben den Limits"),
    (
        "How sessions are found and what's read about them",
        "Wie Sitzungen gefunden werden und was über sie gelesen wird",
    ),
    ("Bubbles and sounds", "Sprechblasen und Töne"),
    (
        "Warnings, the daily budget, the weekly bar and the countdown",
        "Warnungen, das Tagesbudget, der Wochenbalken und der Countdown",
    ),
    (
        "The agents Trayward reads: Claude Code, Codex, Copilot",
        "Die Agenten, die Trayward liest: Claude Code, Codex, Copilot",
    ),
    (
        "Optional features: each adds its settings while it's on",
        "Optionale Funktionen: Jede bringt ihre Einstellungen mit, solange sie an ist",
    ),
    ("Notifications on your phone while you're away", "Benachrichtigungen aufs Handy, während du weg bist"),
    ("Limits and sessions for Home Assistant", "Limits und Sitzungen für Home Assistant"),
    ("Mail between sessions in a shared folder", "Post zwischen Sitzungen in einem gemeinsamen Ordner"),
    (
        "Trayward starts when you sign in. If you move the exe, start it once from its new place and the entry follows.",
        "Trayward startet bei der Anmeldung. Verschiebst du die exe, starte sie einmal am neuen Ort, dann zieht der Eintrag mit.",
    ),
    (
        "How often Trayward asks Anthropic for your limits; when the service says to wait, it waits longer. ↻ in the flyout updates at once.",
        "Wie oft Trayward Anthropic nach deinen Limits fragt; wird es gebremst, wartet es länger. ↻ im Flyout aktualisiert sofort.",
    ),
    (
        "Off: no glow or shake for sessions that need you, and no spinner. Colours and counts still tell you the same.",
        "Aus: kein Leuchten oder Wackeln bei Sitzungen, die dich brauchen, und kein Kreisel. Farben und Zahlen sagen weiter dasselbe.",
    ),
    (
        "A thin line under the widget shows how long your GPG key stays unlocked. Only when git signs commits (commit.gpgsign); the card says what's missing.",
        "Eine dünne Linie unter dem Widget zeigt, wie lange dein GPG-Schlüssel entsperrt bleibt. Nur wenn git Commits signiert (commit.gpgsign); die Karte zeigt, was fehlt.",
    ),
    (
        "Five-hour windows, each allowed a seventh of the weekly limit. Changes nothing on plans with a real 5-hour limit.",
        "Fünf-Stunden-Fenster mit je einem Siebtel des Wochenlimits. Ändert nichts bei Plänen mit echtem 5-Stunden-Limit.",
    ),
    (
        "System: German if Windows' display language is German, English otherwise. Changes at once, no restart.",
        "System: Deutsch, wenn die Anzeigesprache von Windows Deutsch ist, sonst Englisch. Wechselt sofort, ohne Neustart.",
    ),
    (
        "Widgets: over Windows' Widgets button, where app buttons never run under it. Needs Widgets on in Windows' taskbar settings.",
        "Widgets: über der Widgets-Schaltfläche, wo keine Apps darunter laufen. Dafür Widgets in den Taskleisten-Einstellungen einschalten.",
    ),
    (
        "Slim makes the widget narrower, for when it covers taskbar buttons. Also in the right-click menu.",
        "Schmal macht das Widget schmaler, falls es Taskleisten-Schaltflächen verdeckt. Auch im Rechtsklick-Menü.",
    ),
    (
        "Seconds per limit. Slim: all limits take turns. Off: Normal keeps the 5-hour limit, Slim the weekly one.",
        "Sekunden pro Limit. Schmal: alle Limits im Wechsel. Aus: Normal zeigt das 5-Stunden-Limit, Schmal das Wochenlimit.",
    ),
    (
        "The small usage bars in the widget. Never makes it narrower; Always shows them even when taskbar buttons run underneath.",
        "Die kleinen Nutzungsbalken im Widget. Nie macht es schmaler; Immer zeigt sie, auch wenn Schaltflächen darunter laufen.",
    ),
    (
        "Taskbar buttons under the widget are hidden instead of showing through. Rarely needed when it sits on the Widgets button.",
        "Schaltflächen unter dem Widget werden verdeckt statt durchzuscheinen. Selten nötig, wenn es auf der Widgets-Schaltfläche sitzt.",
    ),
    (
        "All includes displays connected later. Windows needs “Show my taskbar on all displays” on (Taskbar behaviors).",
        "Alle schließt später verbundene Bildschirme ein. In Windows muss „Meine Taskleiste auf allen Anzeigen anzeigen“ an sein.",
    ),
    (
        "CPU, memory, GPU, network and disk next to the limits. Read once a second, only while on; nothing leaves the PC.",
        "CPU, Speicher, GPU, Netzwerk und Datenträger neben den Limits. Nur wenn an, einmal pro Sekunde gelesen; nichts verlässt den PC.",
    ),
    (
        "How busy the processor was over the last second, in percent. The tooltip shows it too.",
        "Wie ausgelastet der Prozessor in der letzten Sekunde war, in Prozent. Der Tooltip zeigt es auch.",
    ),
    (
        "Physical memory in use, counted as Task Manager does. The tooltip shows used, size and percentage.",
        "Belegter Arbeitsspeicher, gezählt wie im Task-Manager. Der Tooltip zeigt belegt, Größe und Prozent.",
    ),
    (
        "%: the busiest engine, as Task Manager shows it. GB: video memory in use on the card with the most memory.",
        "%: die meistgenutzte Engine wie im Task-Manager. GB: belegter Grafikspeicher der Karte mit dem meisten Speicher.",
    ),
    (
        "Real adapters only: the virtual ones of WSL and Hyper-V are left out, so nothing counts twice.",
        "Nur echte Netzwerkadapter: die virtuellen von WSL und Hyper-V bleiben außen vor, damit nichts doppelt zählt.",
    ),
    (
        "%: busy time, like “active time” in Task Manager. Read/write: bytes per second, in a column of their own.",
        "%: aktive Zeit wie im Task-Manager. Lesen/Schreiben: Bytes pro Sekunde, in einer eigenen Spalte.",
    ),
    (
        "A faint graph behind each value. Off: just the numbers.",
        "Ein blasser Verlauf hinter jedem Wert. Aus: nur die Zahlen.",
    ),
    (
        "Adds Trayward's hooks to Claude Code's settings.json; off removes exactly those. Open sessions may need a restart.",
        "Fügt Traywards Hooks in die settings.json von Claude Code ein; Aus entfernt genau diese. Offene Sitzungen brauchen evtl. einen Neustart.",
    ),
    (
        "Reads the sessions' transcripts. Off: no titles, cache timers or background work (👀) in the list.",
        "Liest die Transkripte der Sitzungen. Aus: keine Titel, Cache-Timer oder Hintergrundarbeit (👀) in der Liste.",
    ),
    (
        "Claude Code's sessions in WSL, in the list and the statistics. A stopped distribution is never started for it, and an unused one is left to stop.",
        "Claude-Code-Sitzungen in WSL, in der Liste und der Statistik. Eine gestoppte Distribution wird dafür nie gestartet, eine ungenutzte nicht wach gehalten.",
    ),
    (
        "Keeps what the statistics show: numbers, each prompt's start, tool calls with commands, files and errors. Not Claude's replies. The statistics open faster with it. Off deletes what was kept.",
        "Behält, was die Statistik zeigt: Zahlen, den Anfang jedes Prompts, Tool-Aufrufe mit Befehlen, Dateien und Fehlern. Nicht Claudes Antworten. Die Statistik öffnet damit schneller. Ausschalten löscht, was behalten wurde.",
    ),
    (
        "Archived in Claude Code's VS Code extension. Off: they're always listed, faded.",
        "Archiviert in der VS-Code-Erweiterung von Claude Code. Aus: Sie stehen immer in der Liste, blass.",
    ),
    (
        "How often Claude Code's session list is read. With Instant updates on, most changes show at once anyway.",
        "Wie oft die Sitzungsliste von Claude Code gelesen wird. Mit Sofort-Updates kommen die meisten Änderungen ohnehin sofort.",
    ),
    (
        "The switch for all bubbles: limit warnings, pipelines and updates too. None while you present or Do not disturb is on.",
        "Schaltet alle Sprechblasen, auch Limit-Warnungen, Pipelines und Updates. Keine, während du präsentierst oder „Nicht stören“ an ist.",
    ),
    (
        "Custom plays a WAV file of yours. It also plays when a prompt cache is about to run out or a pipeline fails.",
        "Eigener spielt eine WAV-Datei von dir. Er kommt auch, wenn ein Prompt-Cache bald abläuft oder eine Pipeline fehlschlägt.",
    ),
    (
        "Custom plays a WAV file of yours. It also plays when a limit is free again, a pipeline passes or a request is merged.",
        "Eigener spielt eine WAV-Datei von dir. Er kommt auch, wenn ein Limit wieder frei ist, eine Pipeline durchläuft oder gemergt wird.",
    ),
    (
        "A custom WAV plays as loud as it was recorded. Mute sounds in the right-click menu silences them all for now.",
        "Eine eigene WAV spielt so laut, wie sie aufgenommen ist. „Töne stumm“ im Rechtsklick-Menü schaltet vorerst alle aus.",
    ),
    (
        "Each warning comes once per week. Copilot's monthly limit, when shown, warns at the same level.",
        "Jede Warnung kommt einmal pro Woche. Das Monatslimit von Copilot warnt, wenn gezeigt, beim selben Stand.",
    ),
    (
        "Once per 5-hour window. It waits while you present or Do not disturb is on, and comes afterwards.",
        "Einmal pro 5-Stunden-Fenster. Sie wartet, während du präsentierst oder „Nicht stören“ an ist, und kommt danach.",
    ),
    (
        "Each day's share of the week: the days in the flyout's weekly bar and today's limit (D). Days run from midnight. Even: a seventh every day, by its hours.",
        "Der Tagesanteil der Woche: die Tage im Wochenbalken des Flyouts und das Tageslimit (D). Tage beginnen um Mitternacht. Gleich: jeden Tag ein Siebtel, nach seinen Stunden.",
    ),
    (
        "Days: how much of its budget each day used. Budgets: the bar is the week, each day as wide as its budget.",
        "Tage: wie viel seines Budgets jeder Tag verbraucht hat. Budgets: der Balken ist die Woche, jeder Tag so breit wie sein Budget.",
    ),
    (
        "Goes by this week's average use per day; in the week's first hours, by your past weeks.",
        "Richtet sich nach dem Tagesdurchschnitt dieser Woche; in den ersten Stunden der Woche nach den letzten Wochen.",
    ),
    (
        "From that level on, the widget shows how long until the limit resets. Full: only once it's used up.",
        "Ab diesem Stand zeigt das Widget, wie lange es bis zum Reset dauert. Voll: erst, wenn es aufgebraucht ist.",
    ),
    (
        "For the weekly and the 5-hour limit, with the “done” sound.",
        "Für das Wochen- und das 5-Stunden-Limit, mit dem „fertig“-Ton.",
    ),
    (
        "From 08:00 on Monday: prompts, tokens, API value and how long Claude worked. A click opens the statistics' list of weeks.",
        "Ab Montag 08:00: Prompts, Token, API-Wert und wie lange Claude gearbeitet hat. Ein Klick öffnet die Wochen in der Statistik.",
    ),
    (
        "Read with the login Claude Code saved: usage from Anthropic, sessions from your PC. Without ~/.claude nothing is asked.",
        "Gelesen mit der Anmeldung, die Claude Code gespeichert hat: Nutzung von Anthropic, Sitzungen von deinem PC. Ohne ~/.claude wird nichts gefragt.",
    ),
    (
        "Fable's own limit, a share of the weekly one, under Weekly in the flyout and in the tooltip. On by default; only on plans that have one.",
        "Fables eigenes Limit, ein Anteil am Wochenlimit, unter Woche im Flyout und im Tooltip. Standardmäßig an; nur bei Tarifen, die eins haben.",
    ),
    (
        "Read from Codex's own files: no sign-in, no network. Its limits are as of its last use.",
        "Aus den eigenen Dateien von Codex gelesen: ohne Anmeldung, ohne Netzwerk. Seine Limits sind vom letzten Nutzen.",
    ),
    (
        "All: Claude's and Codex's limits, and Copilot's monthly one when shown. Claude or Codex: only theirs.",
        "Alle: die Limits von Claude und Codex, dazu das Monatslimit von Copilot, wenn gezeigt. Claude oder Codex: nur deren.",
    ),
    (
        "With limit asks GitHub every 5 minutes through gh, with its login: install the GitHub CLI and run gh auth login.",
        "Mit Limit fragt alle 5 Minuten über gh bei GitHub nach, mit dessen Anmeldung: GitHub CLI installieren und gh auth login ausführen.",
    ),
    (
        "Each session's MR or PR and its pipeline, through glab and gh: with Trayward's own token (GitLab and GitHub page), else their login.",
        "MR oder PR und Pipeline jeder Sitzung, über glab und gh: mit Traywards eigenem Token (Seite GitLab und GitHub), sonst ihrer Anmeldung.",
    ),
    (
        "GitLab's command-line tool: Trayward runs it for merge requests, with its own token for the host or glab's login.",
        "Das Kommandozeilen-Tool von GitLab: Trayward nutzt es für Merge-Requests, mit eigenem Token für den Host oder der Anmeldung von glab.",
    ),
    (
        "GitHub's command-line tool: Trayward runs it for pull requests (its own token or gh's login) and Copilot's limit (gh's login).",
        "Das Kommandozeilen-Tool von GitHub: Trayward nutzt es für Pull-Requests (eigenes Token oder Anmeldung von gh) und Copilots Limit (Anmeldung von gh).",
    ),
    ("glab and gh, and Trayward's own tokens for them", "glab und gh, und Traywards eigene Tokens dafür"),
    (
        "In the flyout (each session's MR or PR, your own open ones), as bubbles when a pipeline ends or a request is merged, or both.",
        "Im Flyout (MR oder PR jeder Sitzung, deine eigenen offenen), als Sprechblasen, wenn eine Pipeline endet oder gemergt wird, oder beides.",
    ),
    (
        "A read-only fine-grained GitLab token: Merge request, Pipeline and Project: Read; User: Read. Copy it, then Paste token. Click for how to make one.",
        "Ein schreibgeschütztes, fein abgestuftes GitLab-Token: Merge request, Pipeline und Project: Read; User: Read. Kopieren, dann Token einfügen. Klick für die Anleitung.",
    ),
    (
        "A read-only fine-grained GitHub token: Pull requests, Commit statuses and Actions: Read. Copy it, then Paste token. Click for how to make one.",
        "Ein schreibgeschütztes, fein abgestuftes GitHub-Token: Pull requests, Commit statuses und Actions: Read. Kopieren, dann Token einfügen. Klick für die Anleitung.",
    ),
    (
        "With the logo, number and title. Needs Bubble on (Notifications).",
        "Mit Logo, Nummer und Titel. Dafür muss Sprechblase (Benachrichtigungen) an sein.",
    ),
    (
        "Pushes go to your push service. Only the session's name and state go out, never prompts or paths.",
        "Pushes gehen an deine Push-Adresse. Es gehen nur Name und Zustand der Sitzung raus, nie Prompts oder Pfade.",
    ),
    (
        "A locked PC counts as away the same way. Snoozed notifications stop the pushes too.",
        "Ein gesperrter PC zählt genauso als abwesend. Pausierte Benachrichtigungen stoppen auch die Pushes.",
    ),
    (
        "Off: only sessions that need you are pushed. On: finished ones too.",
        "Aus: Nur Sitzungen, die dich brauchen, werden gepusht. An: auch fertige.",
    ),
    (
        "Edit: pick ntfy, Gotify, Discord, Slack or a webhook and fill in what it needs, or paste an address there. Kept encrypted; Test sends one, Remove forgets it.",
        "Bearbeiten: ntfy, Gotify, Discord, Slack oder einen Webhook wählen und ausfüllen, was er braucht, oder dort eine Adresse einfügen. Verschlüsselt; Testen schickt eine, Entfernen vergisst sie.",
    ),
    (
        "For Home Assistant and other MQTT tools: a device with sensors for limits and sessions, and events for automations.",
        "Für Home Assistant und andere MQTT-Werkzeuge: ein Gerät mit Sensoren für Limits und Sitzungen und Ereignisse für Automationen.",
    ),
    (
        "Edit: host, port, user name and password, as Home Assistant shows them, or paste a whole mqtt:// address there. The password is kept encrypted; Remove forgets it all.",
        "Bearbeiten: Host, Port, Benutzername und Passwort, wie Home Assistant sie zeigt, oder dort eine ganze mqtt://-Adresse einfügen. Das Passwort wird verschlüsselt gespeichert; Entfernen vergisst alles.",
    ),
    (
        "Unread and handled mail in the flyout, and ✉ on a session with mail waiting. Trayward only reads the folder.",
        "Ungelesene und erledigte Post im Flyout und ✉ an Sitzungen mit wartender Post. Trayward liest den Ordner nur.",
    ),
    (
        "Looks for .claude-mailbox above your sessions' folders. Elsewhere? In Explorer: Copy as path, then Paste.",
        "Sucht .claude-mailbox über den Ordnern deiner Sitzungen. Woanders? Im Explorer „Als Pfad kopieren“, dann Einfügen.",
    ),
    (
        "It only tells you: nothing is downloaded or installed. Check now asks right away.",
        "Sagt nur Bescheid: Nichts wird heruntergeladen oder installiert. „Jetzt prüfen“ fragt sofort.",
    ),
    ("What your own programs report", "Was deine eigenen Programme melden"),
    (
        "Your own programs write JSON files that Trayward only reads: rows for the tooltip and the flyout. Nothing in them is run, opened or sent.",
        "Deine eigenen Programme schreiben JSON-Dateien, die Trayward nur liest: Zeilen für Tooltip und Flyout. Nichts darin wird ausgeführt, geöffnet oder gesendet.",
    ),
    (
        "Where your programs put their files, one per extension. Each one found gets its switch below, off until you switch it on.",
        "Wo deine Programme ihre Dateien ablegen, eine pro Erweiterung. Jede gefundene bekommt unten ihren Schalter, aus, bis du ihn einschaltest.",
    ),
    (
        "The folder trayward.exe is in. To replace it with a newer one, exit Trayward first. Moved? Start it once from there: Start with Windows and the hooks follow.",
        "Der Ordner, in dem trayward.exe liegt. Zum Ersetzen durch eine neuere zuerst Trayward beenden. Verschoben? Einmal dort starten: Mit Windows starten und die Hooks ziehen nach.",
    ),
    // END DE
];

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
        assert_eq!(url("Usage bars"), "https://github.com/TheDelta/trayward/blob/main/docs/settings.md#usage-bars");
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
        for (_, tip) in PAGES {
            assert!(DE.iter().any(|(en, _)| en == tip), "no German for: {tip}");
        }
        for (_, tip) in TOPICS {
            assert!(DE.iter().any(|(en, _)| en == tip), "no German for: {tip}");
        }
    }
}
