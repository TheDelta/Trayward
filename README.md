> [!NOTE]
> **AI-assisted project.** Trayward was built with AI-assisted development: most of
> the code and documentation was written with an AI coding assistant and reviewed,
> tested and directed by the maintainer.
>
> It's provided as is, and new features and improvements are welcome (see
> [Contributing](#️-contributing) below).

<p align="center"><img src="assets/icon.png" width="96" height="96" alt=""></p>

<h1 align="center">Trayward</h1>

<p align="center">
  <strong>Your Claude Code limits and sessions, right in the Windows 11 taskbar.</strong>
</p>

<p align="center">
  <a href="https://github.com/TheDelta/trayward/actions/workflows/ci.yaml"><img alt="CI" src="https://github.com/TheDelta/trayward/actions/workflows/ci.yaml/badge.svg"></a>
  <a href="https://github.com/TheDelta/trayward/releases"><img alt="Latest release" src="https://img.shields.io/github/v/release/TheDelta/trayward?include_prereleases&sort=semver"></a>
  <img alt="Windows 11" src="https://img.shields.io/badge/Windows-11-0078d4">
  <img alt="Rust 1.85+" src="https://img.shields.io/badge/rust-1.85%2B-b7410e">
  <a href="LICENSE"><img alt="MIT License" src="https://img.shields.io/github/license/TheDelta/trayward"></a>
  <br>
  <a href="https://sonarcloud.io/summary/new_code?id=TheDelta_trayward"><img alt="Quality gate" src="https://sonarcloud.io/api/project_badges/measure?project=TheDelta_trayward&metric=alert_status"></a>
  <a href="https://sonarcloud.io/summary/new_code?id=TheDelta_trayward"><img alt="Coverage" src="https://sonarcloud.io/api/project_badges/measure?project=TheDelta_trayward&metric=coverage"></a>
  <a href="https://scorecard.dev/viewer/?uri=github.com/TheDelta/trayward"><img alt="OpenSSF Scorecard" src="https://api.scorecard.dev/projects/github.com/TheDelta/trayward/badge"></a>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/demo-dark.gif">
    <img src="docs/images/demo-light.gif" alt="Trayward in the taskbar: sessions working, then one finishes and a bubble says it's done" width="760">
  </picture>
</p>

<p align="center"><sub>All pictures show the demo (<code>trayward.exe --demo</code>): made-up sessions and usage.</sub></p>

You use [Claude Code](https://code.claude.com) with a Claude subscription and keep
asking yourself: how much of this week's limit is left, which session is waiting for
me, and is that one done yet? Trayward answers that next to the clock: your weekly
and 5-hour limits, every session that needs you, is done or is still working, and how
long its prompt cache lasts. One click takes you to the session in VS Code.

- **One small exe.** Native Windows, no runtime, no installer, no account. About
  15 MB of memory while it waits.
- **Runs on your PC, tracks nobody.** No telemetry; the connections are to Anthropic,
  for the limits Claude Code's `/usage` shows, with the login Claude Code already
  saved, and once a day to GitHub, to see if there's a newer Trayward (you can switch
  that off).
- **Reads, doesn't touch.** Claude Code's files are only read. The one exception you
  switch on yourself: hooks for instant updates, which Trayward removes again when you
  switch them off.

> Trayward is an independent project. It is **not affiliated with, endorsed by or
> part of [Anthropic](https://www.anthropic.com)**. "Claude" and "Claude Code" are
> trademarks of Anthropic, PBC, used only to describe what Trayward works with.

## ✨ Features

### In the taskbar

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/taskbar-dark.png">
    <img src="docs/images/taskbar-light.png" alt="The widget: dots and counts for sessions that need you, are done or are working, then Claude’s and Codex’s weekly and 5-hour limits" width="208">
  </picture>
</p>

- **Limits at a glance:** weekly (`W`) and 5-hour (`5h`) usage in two rows like the
  clock, amber from 80 % and red from 95 %; the weekly limit also turns amber once you're
  over an even pace, and red when your pace would use it up well before the reset. Plans without a 5-hour limit show the
  plan name instead (or a simulated 5-hour limit, a day's share of the week); Ultra
  gets a badge that shines while you look at it.
- **Needs you, done, working:** a dot column with counts. A session waiting for you
  glows, shakes now and then, and turns red after 10 minutes; a finished session's
  dot turns red when its prompt cache is about to expire.
- **On every taskbar you want:** with several displays, on the main taskbar, on all
  of them, or on the taskbars of the displays you choose.
- **Pin a session:** the pin on a session in the flyout puts it into the taskbar too,
  with its state or its cache timer, so the one you're waiting for is always in view.
- **A tooltip** when you rest the mouse on it: both limits with their reset times,
  the plan and the sessions, without opening anything.

### The flyout

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/flyout-dark.png">
    <img src="docs/images/flyout-light.png" alt="The flyout: plan, weekly budget split into days, 5-hour limit, Codex's limits and the session list" width="420">
  </picture>
</p>

- **Weekly budget:** the weekly bar split into days, with today's use, where an even
  pace would be and where your pace leads, green on pace, yellow over it, red when it
  runs out before the reset; reset times for both limits. Point at a day for its
  budget and use, green within it and red over it. Today's limit (`D`) takes turns
  with the 5-hour limit in the taskbar. The flyout's title opens your usage page on
  claude.ai (with what Trayward can't read, such as limit resets).
- **Every session, on Windows and in WSL:** title, status, VS Code window, model,
  context size, tokens written, running agents, and a 🕒 timer for the prompt cache
  (answer before it runs out and Claude doesn't have to re-read the whole
  conversation).
- 👀 **Watching:** a session that finished its turn but left something running in
  the background (a command, a monitor, a background agent, a scheduled wake-up) says
  so.
- **Done until you look:** a finished session stays "Done" until you click it or
  have its VS Code window in front for a moment, so you know where to look next.
- **Context getting full:** a session warns before Claude Code compacts its
  conversation automatically.
- **Jump to a session:** a click opens its tab in VS Code, or brings its terminal to
  the front (in Windows Terminal, on the session's tab).

### Notifications

- A small bubble and a soft sound when a session needs you or is done: three built-in
  sounds each, or your own WAV file.
- A warning once when the weekly limit reaches 80 % and 95 %, the 5-hour limit 90 %,
  or your pace would use up the week early; the levels are yours to choose.
- **Countdown:** once a limit is nearly used up, the widget shows the time until it
  resets instead of the percentage; and when it resets, a bubble says it's free again.
- **Your week:** on Monday morning a bubble sums up the week before: prompts, tokens
  written, API value and how long Claude worked. A click opens the statistics.
- **Away mode:** step away from the PC and a session that needs you (or is done) goes
  to your phone too: through [ntfy](https://ntfy.sh), a Discord or Slack webhook, or any
  address that takes JSON. Off until you switch it on (Settings → **Away**); only the
  session's name and state go out.
- **Quiet when you need it:** no sounds or bubbles while you present, play full
  screen or have Windows' **Do not disturb** on, or for as long as you snooze them
  (right-click → **Snooze notifications**). Limit warnings wait until then; the
  widget's dots still tell you.

### Codex too

- If you also use OpenAI's **Codex CLI**, Trayward shows its 5-hour and weekly limits
  next to Claude's (each with a small mark), its sessions in the same list, and says
  when one is done or waits for you. It reads Codex's own files: no sign-in, no
  network. Settings → **Providers** switches it off or picks whose limits the taskbar
  shows.

### Copilot too

- If you also use **GitHub Copilot**'s agent (the Copilot CLI, or the agent in VS
  Code), Trayward shows its sessions in the same list and says when one is done, asks
  you something or waits for a permission. They're in the statistics too. It reads
  Copilot's own files: no sign-in, no network.
- **Monthly limit (optional):** Settings → **Providers** → Copilot → **With limit**
  shows how much of the month's premium requests (or, on the free plan, chat messages)
  is used, in the taskbar, the flyout and the tooltip. Trayward asks GitHub for it
  through `gh`, with gh's own login.

### Session mailbox (opt-in)

- For a particular setup: Claude Code sessions that can't message each other (one in
  Windows, one in WSL) and leave Markdown messages in each other's inbox folders
  instead. Off by default; Settings → **Mailbox** switches it on. The flyout then shows
  what's unread for whom and what was handled, and a session with mail waiting gets a
  ✉ on its row. Trayward only reads the folder. See
  [docs/mailbox.md](docs/mailbox.md).

### Merge requests and pipelines

- Optional (off until you switch it on): each session's branch shows its merge or
  pull request and the pipeline's state, e.g. `!37 · Pipeline running` (with a short title when the session's own says something else); a click opens
  it. The flyout lists your own open merge and pull requests in the projects your
  sessions work in, failed pipelines first.
  A bubble with its logo, number and title says when a pipeline passes, fails or is canceled, or a request is merged.
- From GitLab and GitHub through their own tools, `glab` and `gh`, with the logins
  they already have: Trayward never sees a token.

### Home Assistant and MQTT

- Optional (off until you switch it on): Trayward publishes your limits, plan and
  sessions to an MQTT broker, and an event when a session needs you or is done, a limit
  gets close or is free again.
- **Home Assistant** finds it by itself (MQTT discovery): a "Trayward" device with the
  weekly and 5-hour limits, when they reset, how many sessions need you or work, a
  "Needs you" sensor and a "Session" event for automations. No add-on needed, just
  the MQTT integration with a broker such as the Mosquitto add-on.
- Settings → **MQTT**: copy `mqtt://user:password@host:1883` (or `mqtts://…:8883` for
  TLS), press **Paste**, switch it on. The password is kept encrypted for your Windows
  account.

A notification on your phone when a session needs you:

```yaml
automation:
  - alias: Claude needs me
    triggers:
      - trigger: state
        entity_id: event.trayward_my_pc_session
    conditions:
      - condition: template
        value_template: "{{ trigger.to_state.attributes.event_type == 'needs_you' }}"
    actions:
      - action: notify.mobile_app_my_phone
        data:
          message: "{{ trigger.to_state.attributes.text }}"
```

### Statistics

- Today, 7 days, 30 days or all time, compared with the period before: prompts,
  tokens, what it would cost at API prices, turns and time worked, when you work,
  tools, commands, failures grouped by kind, files changed, every session including
  past and archived ones, and the disk space Claude Code uses.
- **Agent map:** a session and the subagents it started, which are still running,
  and what each one did.
- **Starting context:** how much of the context a session uses before your first
  prompt, split into system prompt, skills, MCP servers, agents, CLAUDE.md and
  memory, over time; and how many skills, MCP servers, agent types, instruction
  files and memory files you have.
- Search, sort, and export to CSV. A session's page copies the command that resumes
  it in a terminal.
- **Kept for good:** Claude Code deletes transcripts after 30 days; Trayward keeps
  their numbers, so "all time" stays all time (Settings → Sessions → **Keep
  statistics**).
- **GPG key timer:** if git signs your commits, how long the key stays unlocked.
- **System monitor (optional):** CPU, memory, GPU, network and disk use next to the
  limits, each switched on its own, with a small graph behind; memory and video memory
  in % or GB, the disk as busy time or read/write rates, all of it in detail in the
  tooltip (off by default; Settings → System monitor, one switch for all of it).

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/statistics-dark.png">
    <img src="docs/images/statistics-light.png" alt="Statistics for the last 7 days: prompts, tokens, API value and turns compared with the week before, tokens per day and per project" width="900">
  </picture>
</p>

## 📋 Requirements

- **Windows 11** with the taskbar at the bottom.
- **Claude Code**, signed in with your Claude subscription. Trayward reads the login
  Claude Code saved; it doesn't need an API key.
- **To jump to sessions:** Claude Code's VS Code extension, or a terminal on Windows
  (Windows Terminal or the classic console; not in WSL).

## 🚀 Install

1. Download `trayward.exe` from the
   [latest release](https://github.com/TheDelta/trayward/releases/latest) and put it
   somewhere permanent, e.g. `%LOCALAPPDATA%\Programs\Trayward\`.
2. Start it. The widget appears to the left of the tray icons.
3. Right-click it → **Settings…** → **Start with Windows**.
4. Optional: **Settings → Sessions → Instant updates** for immediate status changes
   and agent counts (see [hooks](docs/how-it-works.md#instant-updates-hooks)).

The exe is not code-signed yet. Windows SmartScreen may warn the first time (**More
info → Run anyway**), and with **Smart App Control** on, Windows may refuse to start
it at all; then build it yourself (below). Each release lists the exe's SHA-256 sum
and, with `gh attestation verify trayward.exe --owner TheDelta`, you can check that
GitHub built it from this repository.

### Try it first

`trayward.exe --demo` shows made-up projects, sessions, usage and statistics,
generated into `%TEMP%\trayward-demo`. It doesn't read your Claude Code login or
files, doesn't connect to anything and leaves your saved state alone; clicking a
session doesn't open VS Code. Right-click → **Demo plan** switches between Max and
Ultra. Exit the normal Trayward first, or both widgets share
the taskbar.

### Build from source

Needs [Rust](https://rustup.rs/) 1.85 or newer (MSVC toolchain) and the Windows SDK
(part of the Visual Studio Build Tools):

```powershell
git clone https://github.com/TheDelta/trayward.git
cd trayward
cargo build --release
.\target\release\trayward.exe
```

## 🖱️ Using it

| Action                                | What it does                                                                                                            |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| Click the widget                      | The flyout: plan, limits with reset times, weekly budget, sessions                                                      |
| Rest the mouse on the widget          | A tooltip: limits with reset times, plan, sessions                                                                      |
| Click a session                       | Opens it in VS Code or its terminal and marks it as seen (finished sessions stay listed for an hour)                    |
| **×** on a session                    | Hides it until you write in it or it needs you                                                                          |
| Pin on a session                      | Shows it in the taskbar too; again to unpin                                                                             |
| **↻**                                 | Updates usage and sessions now (then rests 15 s: the usage service is rate-limited)                                     |
| Chart button                          | Statistics, for all sessions or (on a session) for that one                                                             |
| **⚙**, or right-click → **Settings…** | Settings                                                                                                                |
| Right-click                           | Refresh now, display mode (normal or slim), mute sounds, bubbles on or off, snooze notifications, settings, about, exit |

| Settings page | What you can set                                                                                                                                                                                                               |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| General       | Start with Windows · refresh every 1 to 15 minutes · animations · GPG key timer · simulate a 5-hour limit                                                                                                                      |
| Taskbar       | Display mode: normal or slim · limits take turns: off, 5 to 30 s · usage bars: automatic (only when there's room), always, never · solid background · which taskbars show the widget                                           |
| Sessions      | Instant updates (hooks) · session details · sessions in WSL · keep statistics · hide archived sessions · how often to check                                                                                                    |
| Notifications | Bubble · "needs you" and "done" sounds (built-in, your WAV, off) · volume                                                                                                                                                      |
| Providers     | Codex on or off · Copilot: off, sessions, or with its monthly limit · whose limits the taskbar shows: all, Claude or Codex · merge and pull requests (glab, gh) · pipeline bubbles                                             |
| Limits        | When to warn: the weekly limit (off, 70, 80 or 90 %, and 95 %) · the 5-hour limit (off, 75, 90 or 95 %) · daily budget: adaptive or even · the pace · countdown from 90 %, 95 % or full · "free again" bubble · weekly summary |
| Away          | Away mode · after 5 to 30 minutes without input · "done" too · the push address (paste, test)                                                                                                                                  |
| Mailbox       | Off by default · its folder: found, or pasted                                                                                                                                                                                  |
| MQTT          | On or off · the broker (paste, test); Home Assistant finds the entities by itself                                                                                                                                              |
| About         | Version and the update check · who made it · source code, report a problem                                                                                                                                                     |

## 🔒 Privacy and security

Trayward runs entirely on your PC. It has no telemetry, no analytics and no account
of its own.

- **Network:** only `api.anthropic.com`, to read your usage and plan
  (`/api/oauth/usage`, `/api/oauth/profile`), with the login Claude Code saved.
  With **Merge and pull requests** switched on, it also starts `glab` and `gh`, which
  ask your GitLab and GitHub about the sessions' branches and your own requests. With
  Copilot's **monthly limit** switched on, it starts `gh` to ask GitHub for it. With
  **Away mode** on, it sends "shop needs you" or "shop is done" to the push address you
  pasted, and nothing else. With **MQTT** on, it sends limits, session names and states
  to your broker.
- **Update check:** once a day, `api.github.com` is asked for Trayward's newest release
  (Settings → About; switch it off there). It only tells you: nothing is downloaded or
  installed.
- **Your login** (`~/.claude/.credentials.json`) is read, never written or sent
  anywhere else.
- **Reads** Claude Code's session list, transcripts and IDE files under `~/.claude`
  (and the same in running WSL distributions), VS Code's settings and its record of
  archived sessions, Codex's session files under `~/.codex` and Copilot's under
  `~/.copilot` (when you use them), and,
  for the GPG timer, your git and GnuPG settings. All read-only.
- **Writes** only its own state (`%LOCALAPPDATA%\Trayward\state.json`), kept
  statistics (`%LOCALAPPDATA%\Trayward\kept`, while **Keep statistics** is on), its
  preferences (`HKCU\Software\Trayward`), the "Start with Windows" entry, CSV files you
  export, and (only if you turn on Instant updates) its own hook entries in
  `~/.claude/settings.json`. Turning them off removes exactly those entries.
- **Starts** VS Code (to open a session), Explorer (to show files), `wsl.exe` (to list
  running distributions), `glab` and `gh` (if you switch merge requests on) and, for the GPG timer, `git`, `gpg`, `gpgconf` and
  `gpg-connect-agent`.

The usage endpoint and Claude Code's files are internal and undocumented. Trayward
reads them defensively, but a Claude Code update can break parts of it until
Trayward catches up. [docs/how-it-works.md](docs/how-it-works.md) explains where every
number comes from.

## 🗑️ Uninstall

1. Settings → Sessions → turn **Instant updates** off (removes the hooks), and
   Settings → General → turn **Start with Windows** off.
2. Exit Trayward (right-click → Exit) and delete `trayward.exe`.
3. Optionally delete `%LOCALAPPDATA%\Trayward` and the registry key
   `HKCU\Software\Trayward`.

## ❓ FAQ

**The widget overlaps my taskbar buttons.** Settings → Taskbar → **Solid background**
covers what runs underneath, **Usage bars: automatic** drops the bars when there's
no room, and **Display mode: Slim** (also in the right-click menu) makes the widget narrower: sessions on top, one limit at a time
below. Windows 11 has no official way to add things to the taskbar, so the widget is
a small window placed inside it.

**It's only on one of my displays.** Settings → Taskbar → **Show on taskbars**: All,
or Choose for single displays. Windows itself must show the taskbar there (Windows
Settings → Personalization → Taskbar → Taskbar behaviors → **Show my taskbar on all
displays**).

**A session doesn't show up.** Sessions appear once Claude Code lists them (a few
seconds after start). WSL sessions need Settings → Sessions → **Sessions in WSL** and
a running distribution.

**Clicking a session asks "Allow 'Claude Code for VS Code' extension to open this
URI?".** Tick **Don't ask again for this extension**; that's how Trayward asks the
extension to reveal the tab.

**Is the "API value" what I pay?** No. It's what the same tokens would cost at
Anthropic's API prices, a way to see how much work went through. Your subscription is
billed differently.

## 🔢 Versions

Trayward follows [Semantic Versioning](https://semver.org/). A new major version means
something you rely on changed: a setting goes away or works differently, the
command-line flags, the hooks it installs, or where it keeps its data. See
[CHANGELOG.md](CHANGELOG.md) for what changed.

## 💬 Questions, ideas and bugs

- **Questions and help:** please use
  [Discussions → Q&A](https://github.com/TheDelta/trayward/discussions/categories/q-a)
  rather than an issue.
- **Ideas:** open a [feature request](https://github.com/TheDelta/trayward/issues/new?template=feature_request.yaml)
  or start a discussion.
- **Bugs:** open a [bug report](https://github.com/TheDelta/trayward/issues/new?template=bug_report.yaml).
- **Security issues:** please report them privately, see [SECURITY.md](SECURITY.md).

## 🛠️ Contributing

Contributions are welcome! [CONTRIBUTING.md](CONTRIBUTING.md) explains the setup, the
checks and how to propose a change, and [AGENTS.md](AGENTS.md) describes how Trayward
works inside (also meant for AI coding agents). Please follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

## 📄 License

Made by [Alex 'Delta' Diller](https://github.com/TheDelta).

[MIT](LICENSE). Inspired by
[claude-usage-widget-for-windows-taskbar](https://github.com/AbdoSarmini/claude-usage-widget-for-windows-taskbar)
(MIT), which showed how to place a widget in the Windows 11 taskbar and where Claude
Code's usage numbers come from. The Rust crates Trayward is built with are listed in
`Cargo.toml`; their licenses are MIT or Apache-2.0.

Your usage numbers, sessions and transcripts come from Anthropic and Claude Code on
your own PC. Trayward doesn't send or store them anywhere else.
