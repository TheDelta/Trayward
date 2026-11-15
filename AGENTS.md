# AGENTS.md: Trayward

Guidance for AI coding agents and contributors: how Trayward works, the decisions
behind it, and how to change it safely.

## What Trayward is

A small native Windows 11 app (Rust, Win32, Direct2D) that shows Claude Code's usage
limits and sessions in the taskbar: a widget next to the clock, a flyout with limits
and sessions, notification bubbles and sounds, a statistics window and a settings
window. The name: it sits in the taskbar, next to the tray, and keeps watch.
"Claude" stays out of the name (Anthropic's trademark). Trayward is not affiliated
with Anthropic; the docs say so.

## Hard constraints

- **Windows 11 only, one exe.** The `windows` crate and serde; no runtime, no
  installer, no other process running alongside. A new crate needs a good reason and
  an MIT or Apache-2.0 license.
- **Read-only towards Claude Code.** The login (`~/.claude/.credentials.json`) is
  read, never written, logged or shown. The only write to Claude Code's files is
  Trayward's own hook entries in `~/.claude/settings.json`, added and removed exactly
  (`tracking.rs`), keeping the file's key order (serde_json `preserve_order`).
- **The network connections** are `api.anthropic.com` (usage and profile) and, once a
  day, `api.github.com` for the newest release (the update check, `update.rs`; it can
  be switched off and never downloads anything). No telemetry, nothing else. The
  exceptions the user switches on:
  merge and pull requests and Copilot's monthly limit, which run `glab` and `gh` (they
  talk to GitLab and GitHub with their own logins; Trayward never handles a token), and
  away mode, which posts a
  session's name and state to the push address the user pasted (`push.rs`), and MQTT,
  which publishes limits, session names and states to the user's broker (`mqtt.rs`).
- **The UI thread never blocks.** Window procedures turn messages into `Event`s that
  `App::handle` processes one at a time (`app.rs`). Network, transcripts, disk sizes,
  WSL, VS Code's state and UI Automation run on background threads and post results
  back (`win::post_boxed`).
- **Claude Code's files are internal and change between versions.** Parse
  defensively: skip what isn't understood, never fail or panic on it, and add a test
  with a small sample line for every format.
- **No personal data in the repository**: no tokens, project names, paths, window
  titles or screenshots of real sessions. Screenshots come from `--demo` only.

## Layout

```text
src/
  main.rs            entry: the app, the hook mode (trayward-hook), --demo
  app.rs             App state, events, timers, window procedures
  widget.rs          the widget, one per taskbar, and where it sits
  taskbars.rs        the taskbars, their displays, which ones show the widget
  uia.rs             UI Automation: where buttons end, where other taskbars' clocks start
  flyout.rs          limits and sessions
  tip.rs             the widget's tooltip
  plan.rs            how each plan's name looks (Max, Ultra)
  bubble.rs, sound.rs  notifications
  stats.rs           statistics from transcripts (cached per file), prices, disk usage
  stats_window.rs    the statistics window: pages, charts, lists, CSV
  setup.rs           what a session loads (skills, MCP, instructions, memory) and its starting context
  sysmon.rs          the optional system monitor: CPU, memory, GPU, network, disk (off by default)
  keep.rs            kept statistics: sessions' numbers after their transcripts are deleted
  settings.rs        preferences in HKCU\Software\Trayward, "Start with Windows"
  settings_window.rs the settings window
  sessions.rs        session states; how session lists and hook events change them
  collector.rs       background reading: session lists, transcripts, window names
  registry.rs        Claude Code's session lists, on Windows and in WSL
  transcripts.rs     incremental transcript reading (title, model, tokens, agents, watchers)
  jsonl.rs           append-only JSON-lines files, read as they grow (transcripts, Copilot's events)
  usage.rs, http.rs  the login and the usage endpoint over WinHTTP
  forecast.rs        weekly budget per day, pace, simulated 5-hour limit
  hook.rs, tracking.rs  the hook mode and installing the hooks
  vscode.rs, vscode_state.rs  VS Code windows, switching to a session, archived sessions
  terminal.rs        switching to a session in a terminal (console window, Windows Terminal tab)
  codex.rs           Codex CLI: limits and sessions from ~/.codex/sessions (second provider)
  copilot.rs         Copilot's agent: sessions from ~/.copilot/session-state, the monthly limit via gh
  mailbox.rs         the session mailbox (opt-in, off by default): unread and handled messages
  forge.rs           merge/pull requests and pipelines via glab and gh (off by default)
  logo.rs            the GitLab and GitHub logos as vector paths (a small SVG path reader)
  push.rs            away mode: pushes to ntfy, Discord, Slack or a JSON address (off by default)
  update.rs          the update check: GitHub's latest release, once a day
  mqtt.rs            MQTT 3.1.1 client, Home Assistant discovery (off by default)
  tls.rs             TLS over TCP with Windows' SChannel, for mqtts://
  alerts.rs          when limit warnings, "free again" and the countdown come (tested)
  gpg.rs             the GPG signing key's cache
  gfx.rs, theme.rs   Direct2D / DirectWrite drawing, light and dark colours
  hits.rs            clickable areas of the flyout, settings and statistics windows
  store.rs           saved state in %LOCALAPPDATA%\Trayward\state.json
  demo.rs            demo mode: made-up sessions, transcripts, usage and history
  timefmt.rs, win.rs, menu.rs  helpers
build.rs, app.rc, app.manifest, assets/   icon, manifest, version info from Cargo.toml (rc.exe from the Windows SDK)
scripts/             release.py, changelog.py, pr_title.py (Python, stdlib), check.mjs, tools.mjs
scripts/tests/       tests of the release scripts
docs/                how-it-works.md, maintenance.md, mailbox.md, images/ (from the demo)
.github/             workflows (see its README), issue forms, Dependabot
```

## Running

```powershell
npm start                               # closes the running one, builds the release, starts it
npm run demo                            # builds and starts the demo next to it
cargo build --release
.\target\release\trayward.exe          # the real one: only one runs (a mutex)
.\target\release\trayward.exe --demo   # made-up data in %TEMP%\trayward-demo; its own mutex
```

Exit the running Trayward (right-click → Exit, or `npm run stop`) before replacing its exe.
Posting `WM_CLOSE` to its hidden control window (class `Trayward.Control`; what `npm run stop`
does) is the same as Exit; `taskkill` without `/F` doesn't reach that window. The demo
reads nothing of the user's: no login, no transcripts, no WSL, no GPG, no hooks, no
update check, and it doesn't save state. It starts with the user's settings and keeps
its changes in memory (`settings.rs`), so trying a setting in the demo changes nothing. Use it for screenshots and for trying visible changes.

## Claude Code quirks (important)

- **Session lists** (`~/.claude/sessions/*.json`, per process) say which sessions
  run; transcripts (`~/.claude/projects/<dir>/<id>.jsonl`) say what happened. A
  session's pid can be reused: compare the process start time.
- **Transcripts are append-only JSON lines**; read only what was added, through
  `jsonl::Tail` (the session tracker, the statistics and Copilot's events all use it).
  Lines can be cut off while written.
- **Background work** (watchers): started by "Command running in background with
  ID", "Monitor started", "Async agent launched", ScheduleWakeup; ended by a
  `<task-notification>` with that id or by TaskStop / KillShell. Watchers from before
  the process started don't count.
- **Turns** run from a prompt to its last activity, cut at a 30-minute pause; compact
  summaries and transcript-only prompts don't count as prompts.
- **The usage endpoint** is undocumented and rate-limited: back off on 429 (and wait
  at least its `Retry-After`), never refresh more often than the setting.
- **Transcripts repeat lines:** Claude Code sometimes writes earlier lines again later
  in the file. Count replies, tool calls and prompts once, by their ids
  (`transcripts::once_key`).
- **Moved projects** leave a copy of a transcript in two project folders: the newest
  one wins.

## The app's code

- **Drawing:** the widget is a layered window (per-pixel alpha, drawn into a DIB);
  the flyout, bubble, settings and statistics windows draw with Direct2D on an
  `ID2D1HwndRenderTarget` per window (`gfx::paint_window`), kept while the window is
  visible. **In software, not on the GPU:** a GPU target loads the graphics driver into the
  process (~70 MB, ~35 threads on NVIDIA); keep it that way. Text layouts are cached.
  Layout and drawing share one function per window (`render(canvas: Option<…>)`):
  without a canvas it only measures and returns the hit areas. It runs for every
  frame (hover, scrolling): what the statistics window works out from its data
  (tallies, filtered and sorted lists) goes through `stats_window::memo`, which keeps
  it until the data is replaced.
- **Taskbars:** the widget is a child window of each taskbar it's shown on
  (`Shell_TrayWnd`, `Shell_SecondaryTrayWnd`), moved every second to stay left of the
  tray (main) or the clock (others, via UI Automation), and recreated after Explorer
  restarts (`TaskbarCreated`).
- **Settings** live in the registry (`settings.rs`), state in a JSON file
  (`store.rs`). New settings get a default that keeps today's behaviour. The state
  file is written by its own thread (`store::Writer`), a moment after the last change:
  `App::save` only hands it over.
- **Other programs** (`gh`, `glab`, `git`, `gpg`, `wsl`) are started through `win::run`: no
  console window, a time limit, their output read while they run (and kept when the time
  runs out, see `Output::complete`). Always by full path (`win::find_program` searches PATH
  without the exe's own folder, `win::system_program` the Windows folder): Windows would
  otherwise run a file of that name next to `trayward.exe` first.
- **The demo shows nothing of the user's**, not even in the settings it starts with:
  addresses (MQTT broker, push address) read through `settings::private_string`, and its
  control window has its own class, so hooks of real sessions never reach it.
- **The state file must stay readable by newer versions:** every stored struct takes a
  missing field as its default (`#[serde(default)]`), and `store::load` keeps what it can
  read of a damaged file, setting the file aside as `state.bad.json`.
- **HTTP** (`http.rs`) doesn't follow redirects, so a request and its login header only
  reach the host they were meant for.
- **The animation tick** (30 a second) only draws the widget and the open flyout again
  (`App::animate`); everything else follows events and the once-a-second tick.

## Releases and CI

How to use them: [CONTRIBUTING.md](CONTRIBUTING.md),
[.github/workflows/README.md](.github/workflows/README.md). The decisions behind
them: [docs/maintenance.md](docs/maintenance.md). The ones a change must not undo:

- **SemVer:** settings, command-line flags (`--demo`, `trayward-hook`), the hook
  entries and where Trayward keeps its data are contracts. The version in
  `Cargo.toml` is the only one; nobody tags by hand (Prepare release, then the merge
  publishes).
- **`CHANGELOG.md` is generated** from pull request titles (Conventional Commits), so
  the title is what counts; hand-written lines under `[Unreleased]` win.
- **Supply chain:** everything pinned exactly (`Cargo.lock`, npm without ranges,
  Python tools by hash, actions by commit SHA); `--locked` and
  `npm ci --ignore-scripts` in CI; minimal `permissions` per job;
  `pull_request_target` and `workflow_run` workflows never run a pull request's code.
  zizmor checks the workflows. New releases of dependencies wait 3 days (Dependabot
  cooldown, `.npmrc`).
- **`npm run check`** runs every check CI runs; `npm run format` fixes what it can.
- **Renaming a CI job** renames a required check: the `main` ruleset on GitHub must
  change with it.

## Conventions

- **LF everywhere** (`.gitattributes` `* text=auto eol=lf`; only `*.cmd` / `*.bat`
  CRLF), whatever `core.autocrlf` says. `.editorconfig`: UTF-8, LF, final newline, no
  trailing spaces.
- **Formatting is enforced:** rustfmt (`rustfmt.toml`, 120 columns) for Rust,
  Prettier (`.prettierrc.yaml`, 100 columns) for Markdown, YAML, JSON and JavaScript,
  Ruff for Python. Run `npm run format` after editing (the pre-commit hook does it for
  staged files). Re-read a file before patching it: editors format on save.
- **Clippy warnings are errors** in CI. `unsafe` blocks carry a `// SAFETY:` comment.
- **YAML files end in `.yaml`**, all of them.
- **Commits and pull request titles:** Conventional Commits (`feat:`,
  `fix(flyout):`, `docs:`, `chore(deps):`, `feat!:` for breaking changes);
  `commitlint.config.mjs`. Squash merges: the title is the commit on `main`.
- **Windows pitfall:** Python's `open(path, "w")` writes CRLF; scripts that edit repo
  files must pass `newline="\n"`. PowerShell's `Set-Content` adds a BOM in Windows
  PowerShell 5; use `[IO.File]::WriteAllText` or pwsh.
- **Wording in the UI:** plain, short, sentence case; say what happens ("Opens it in
  VS Code"), not how it's implemented.
- Short comments that explain _why_; small helpers. When a change is a design
  decision, record it where its topic lives (here or in `docs/`).

## Testing

All tests run without the network and without Claude Code.

- **Rust** (`cargo test`): parsing (session lists, transcripts, hooks, VS Code state),
  statistics, forecasts, time formatting, settings logic, the demo's transcripts. New
  logic that doesn't need a window gets a test, with a small sample line for every
  file format.
- **Release scripts** (`scripts/tests/`, stdlib `unittest`): version bumps, the
  changelog generator, the release notes, the pull request title help.
- **Visible changes:** run `--demo` and look, in light and dark mode, at 100 % and
  150 % scaling if layout changed. Smart App Control may block a fresh test binary
  (os error 4551): build again with a different optimisation level
  (`$env:CARGO_PROFILE_DEV_OPT_LEVEL = 1`).
- **Screenshots for the docs** (`docs/images/`) come from `--demo` only.

## Known limitations

- Windows 11 only, taskbar at the bottom. There's no supported API for taskbar
  widgets: the widget is a child window of Explorer's taskbar.
- The usage endpoint and Claude Code's files are undocumented; a Claude Code update
  can break parts until Trayward catches up.
- The exe isn't code-signed yet: SmartScreen warns, Smart App Control may block it.
- Jumping to a session needs Claude Code's VS Code extension.

## More detail

| File                                                       | What's in it                                                                                                                        |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| [docs/how-it-works.md](docs/how-it-works.md)               | Where every number comes from: limits, sessions, hooks, VS Code, statistics, forecast, GPG, sounds, taskbars; the source map        |
| [docs/maintenance.md](docs/maintenance.md)                 | Versions, the changelog generator, the release flow and why it's built so, CI, Dependabot, SonarQube Cloud, rulesets, tool settings |
| [.github/workflows/README.md](.github/workflows/README.md) | Each workflow, and what to do when it fails                                                                                         |

Read the parts that touch your change first: a change to what Trayward reads starts
with `docs/how-it-works.md`, one to the release flow with `docs/maintenance.md`.
