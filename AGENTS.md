# AGENTS.md: Trayward

Guidance for AI coding agents and contributors: how Trayward works, the decisions
behind it, and how to change it safely.

## What Trayward is

A small native taskbar app (Rust) that shows AI coding agents' usage limits and
sessions: a widget next to the clock, a flyout with limits and sessions, notification
bubbles and sounds, a statistics window and a settings window. It was made first for
Claude Code on Windows 11 (Win32, Direct2D), and those stay the most complete. It's
built to take more: Codex CLI and GitHub Copilot's agent are further providers, more
can follow, and so can other operating systems (see "Providers and platforms" below). The name: it sits in the taskbar, next to the tray, and keeps watch.
"Claude" stays out of the name (Anthropic's trademark). Trayward is not affiliated
with Anthropic or anyone else whose products it works with (OpenAI, GitHub, Microsoft,
GitLab); the README and the About page say so, and whose trademarks the names are.

## Hard constraints

- **One native exe.** On Windows 11, the one platform today: the `windows` crate and
  serde; no runtime, no installer, no other process running alongside (the exe starts itself for a
  moment where a second process is the only safe way: the hook mode, and looking at a terminal,
  `terminal.rs`). A new crate needs a good reason and
  an MIT or Apache-2.0 license.
- **Read-only towards the agents.** Their files and logins (such as Claude Code's
  `~/.claude/.credentials.json`) are read, never written, logged or shown. The only write to Claude Code's files is
  Trayward's own hook entries in `~/.claude/settings.json`, added and removed exactly
  (`core/src/tracking.rs`), keeping the file's key order (serde_json `preserve_order`).
- **The network connections** are `api.anthropic.com` (usage and profile; only when
  Claude Code is used on the PC, `~/.claude` exists) and, once a
  day, `api.github.com` for the newest release (the update check, `update.rs`; it can
  be switched off and never downloads anything). No telemetry, nothing else. The
  exceptions the user switches on:
  merge and pull requests and Copilot's monthly limit, which run `glab` and `gh` (they
  talk to GitLab and GitHub: merge and pull requests with the token the user pasted for
  Trayward or else the tool's own login, Copilot's limit always with gh's own login), and
  away mode, which posts a
  session's name and state to the push service the user set up (`push.rs`), and MQTT,
  which publishes limits, session names and states to the user's broker (`mqtt.rs`).
- **The UI thread never blocks.** Window procedures turn messages into `Event`s that
  `App::handle` processes one at a time (`app.rs`). Network, transcripts, disk sizes,
  WSL, VS Code's state and UI Automation run on background threads and post results
  back (`win::post_boxed`; the core's readers take a callback, `win::poster`).
- **Claude Code's files are internal and change between versions.** Parse
  defensively: skip what isn't understood, never fail or panic on it, and add a test
  with a small sample line for every format.
- **No personal data in the repository**: no tokens, project names, paths, window
  titles or screenshots of real sessions. Screenshots come from `--demo` only.
- **Tokens the user gives Trayward** (for `glab` and `gh`) live only in Windows Credential
  Manager (`os::secret`), go to the tool only as an environment variable of its process (never
  as an argument), and are never written elsewhere, logged or shown; errors pass through
  `forge::redact`. Trayward never reads another program's stored logins.

## Providers and platforms

Trayward started with Claude Code on Windows; neither is meant to be the only one.

- **A provider** (an AI coding agent) is its own module that reads the agent's own
  files or API (`codex.rs`, `copilot.rs`; Claude Code's is spread over `usage.rs`,
  `registry.rs`, `transcripts.rs`, `hook.rs`) and maps them to the shared types: limits
  (`usage::Limit`), sessions and their states (`sessions::Status`). It gets a setting
  (on by default when the agent is found on the PC), its rows in the flyout and the
  tooltip, and MQTT keys that start with its name. Its parsing follows the rules for
  Claude Code's files below: defensive, with a sample line in a test.
- **A platform** (an operating system): Windows is the one today. The logic lives in
  the core crate (`core/`): providers, parsing, sessions, forecasts, alerts,
  statistics, MQTT, push, the update check. It calls no system API itself: what it
  needs from the system (processes, running programs, local time, the display
  language, HTTPS, TLS) goes through `core/src/os/`, with Windows in `os/windows/` and
  fallbacks for any other system in `os/other.rs`, so the core builds and its tests run
  there too (CI's "Core on Linux"). The Windows app (`src/`) has the rest: windows and
  drawing (`win.rs`, `gfx.rs`, the windows' procedures in `app.rs`), the taskbar
  (`taskbars.rs`, `uia.rs`, `widget.rs`'s placing), settings in the registry
  (`settings.rs`), DPAPI, VS Code and terminals. New logic goes into the core; another
  system gets its own `os/` module and its own app. Wording in the docs and the UI
  says "the taskbar", "your PC", not "Windows" where it isn't Windows-specific.

## Layout

Two crates in one Cargo workspace: the Windows app (the root package, `src/`) and the
core (`core/`, the crate `trayward-core`), the logic without the UI. The app uses the
core's modules under their own names (`crate::sessions`, `crate::usage`: see `main.rs`).

```text
src/                 the Windows app
  main.rs            entry: the app, the hook mode (trayward-hook), the terminal helper (trayward-terminal), --demo
  app.rs             App state, events, timers, window procedures
  widget.rs          the widget, one per taskbar, and where it sits
  taskbars.rs        the taskbars, their displays, which ones show the widget
  uia.rs             UI Automation: where buttons end, where other taskbars' clocks start
  flyout.rs          limits and sessions
  tip.rs             the widget's tooltip
  plan.rs            how each plan's name looks (Max, Ultra)
  bubble.rs, sound.rs  notifications
  stats_window.rs    the statistics window: pages, charts, lists, CSV
  sysmon.rs          the optional system monitor: CPU, memory, GPU, network, disk (off by default)
  settings.rs        preferences in HKCU\Software\Trayward, "Start with Windows"
  settings_window.rs the settings window
  edit.rs            its text fields: Windows' own edit controls over the drawn boxes
  help.rs            the settings' "?": short help on hover, docs/settings.md on click
  i18n.rs, i18n/de.rs  the app's translations: all its German in de.rs, on top of the core's
  collector.rs       background reading: session lists, transcripts, window names
  vscode.rs, vscode_state.rs  VS Code windows, switching to a session, archived sessions
  terminal.rs        switching to a session in a terminal (console window, Windows Terminal tab), and the
                     helper process that attaches to its console
  logo.rs            the GitLab and GitHub logos as vector paths (a small SVG path reader)
  gfx.rs, theme.rs   Direct2D / DirectWrite drawing, light and dark colours
  hits.rs            clickable areas of the flyout, settings and statistics windows
  win.rs, menu.rs    Win32 helpers (window messages, clipboard, the hook's delivery), the menu
core/src/            the core (trayward-core): no UI, nothing Windows-only outside os/
  lib.rs             what's in it
  os/                what it asks of the system: mod.rs (the clock, running and finding programs),
                     windows/ (processes, local time, language, folder changes, HTTPS over WinHTTP,
                     TLS over SChannel, secrets in Credential Manager),
                     other.rs (fallbacks elsewhere, so the core builds and its tests run there)
  i18n.rs, i18n/de.rs  English and German: the language; all the core's German in de.rs
  stats.rs           statistics from transcripts (cached per file), prices, disk usage
  setup.rs           what a session loads (skills, MCP, instructions, memory) and its starting context
  keep.rs            kept statistics: sessions' numbers after their transcripts are deleted, and where
                     reading a transcript goes on the next time
  sessions.rs        session states; how session lists and hook events change them
  registry.rs        Claude Code's session lists, on Windows and in WSL
  transcripts.rs     incremental transcript reading (title, model, tokens, agents, watchers)
  jsonl.rs           append-only JSON-lines files, read as they grow (transcripts, Copilot's events)
  usage.rs           the login and the usage endpoint
  forecast.rs        weekly budget per day, pace, simulated 5-hour limit
  hook.rs, tracking.rs  the hook mode (the app delivers its message) and installing the hooks
  codex.rs           Codex CLI: limits and sessions from ~/.codex/sessions (second provider)
  copilot.rs         Copilot's agent: sessions from ~/.copilot/session-state, the monthly limit via gh
  mailbox.rs         the session mailbox (opt-in, off by default): unread and handled messages
  extensions.rs      the user's own extensions (off by default): JSON files read, never run (docs/extensions.md)
  forge.rs           merge/pull requests and pipelines via glab and gh (off by default)
  push.rs            away mode: pushes to ntfy, Gotify, Discord, Slack or a JSON address (off by default)
  update.rs          the update check: GitHub's latest release, once a day
  mqtt.rs            MQTT 3.1.1 client, Home Assistant discovery (off by default)
  alerts.rs          when limit warnings, "free again" and the countdown come (tested)
  gpg.rs             the GPG signing key's cache
  store.rs           saved state in %LOCALAPPDATA%\Trayward\state.json
  demo.rs            demo mode: made-up sessions, transcripts, usage and history
  timefmt.rs         time formats
build.rs, app.rc, app.manifest, assets/   icon, manifest, version info from Cargo.toml (rc.exe from the Windows SDK)
.cargo/config.toml   the C runtime linked into the exe (no Visual C++ runtime to install)
scripts/             release.py, changelog.py, pr_title.py, licenses.py (Python, stdlib), check.mjs, tools.mjs, run.mjs
                     (npm start, stop, demo), extension-example.mjs (a sample extension file)
scripts/tests/       tests of the release scripts
fuzz/                the fuzzer's targets (cargo-fuzz, its own workspace) and their seeds
docs/                settings.md (the users' help), how-it-works.md, maintenance.md, mailbox.md, extensions.md,
                     images/ (from the demo)
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
- **A fork's transcript** (a subagent of the type `fork`) starts with a
  `fork-context-ref` line and the reply of the session that started it, under the same
  id: that reply is the session's, not the fork's (`stats.rs`).
- **Counting transcripts differently** (a fix in `Stats::line`, or something new read into
  `Stats`) needs `stats::PARSER` raised: kept statistics say which number counted them, and
  the ones from another number are made again from their transcripts.

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
  tray (main) or the clock (others, via UI Automation), or, with Position "Widgets",
  onto Windows' Widgets button (UI Automation id `WidgetsButton`; Windows keeps the app
  buttons away from it, the one way to have reserved room: Windows has no API for it),
  and recreated after Explorer restarts (`TaskbarCreated`).
- **Settings** live in the registry (`settings.rs`), state in a JSON file
  (`store.rs`). Every settings card has help: a `### <card title>` section in
  `docs/settings.md` and its short text in `help.rs` (its German in `i18n/de.rs`); a test
  checks that both match. New settings get a default that keeps today's behaviour. The state
  file is written by its own thread (`store::Writer`), a moment after the last change:
  `App::save` only hands it over.
- **Other programs** (`gh`, `glab`, `git`, `gpg`, `wsl`) are started through `os::run`: no
  console window, a time limit, their output read while they run (and kept when the time
  runs out, see `Output::complete`). Always by full path (`os::find_program` searches PATH
  without the exe's own folder, `os::system_program` the Windows folder): Windows would
  otherwise run a file of that name next to `trayward.exe` first.
- **The demo shows nothing of the user's**, not even in the settings it starts with:
  addresses (MQTT broker, push service) read through `settings::private_string`, and its
  control window has its own class, so hooks of real sessions never reach it.
- **The state file must stay readable by newer versions:** every stored struct takes a
  missing field as its default (`#[serde(default)]`), and `store::load` keeps what it can
  read of a damaged file, setting the file aside as `state.bad.json`.
- **HTTP** (`os::http`) doesn't follow redirects, so a request and its login header only
  reach the host they were meant for.
- **The animation tick** (30 a second) only draws the widget and the open flyout again
  (`App::animate`); everything else follows events and the once-a-second tick.

## Releases and CI

How to use them: [CONTRIBUTING.md](CONTRIBUTING.md),
[.github/workflows/README.md](.github/workflows/README.md). The decisions behind
them: [docs/maintenance.md](docs/maintenance.md). The ones a change must not undo:

- **SemVer:** settings, command-line flags (`--demo`, `trayward-hook`), the hook
  entries and where Trayward keeps its data are contracts (`trayward-terminal` is not: only
  Trayward starts itself with it). The version in
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
- **Clippy warnings are errors** in CI, with a few lints beyond its default set
  (`[workspace.lints]` in `Cargo.toml`): `unsafe` blocks carry a `// SAFETY:` comment; no
  `unwrap`, `expect`, `panic`, `dbg!`, `todo!` or printing outside tests (`clippy.toml`
  lets tests); no text cut by position (`&text[a..b]`, tests included: `get`, `split_once`,
  `strip_prefix` instead), and lists read from outside aren't cut or indexed that way
  either, because the release build ends the app on any panic; every `#[allow(…)]` gives a
  `reason`. Doc comments have no broken links
  (`cargo doc` fails on them). cargo-deny (`deny.toml`) checks the crates: RustSec
  advisories, MIT or Apache-2.0 (two crates other crates bring in are allowed by name),
  crates.io only.
- **YAML files end in `.yaml`**, all of them.
- **Commits and pull request titles:** Conventional Commits (`feat:`,
  `fix(flyout):`, `docs:`, `chore(deps):`, `feat!:` for breaking changes);
  `commitlint.config.mjs`. Squash merges: the title is the commit on `main`.
- **Windows pitfall:** Python's `open(path, "w")` writes CRLF; scripts that edit repo
  files must pass `newline="\n"`. PowerShell's `Set-Content` adds a BOM in Windows
  PowerShell 5; use `[IO.File]::WriteAllText` or pwsh.
- **Translations:** the code holds the English (`t("…")`, `f("…", …)`); the German of all of
  it is in one file per crate, `src/i18n/de.rs` and `core/src/i18n/de.rs`, grouped by the file
  the texts are from. A test fails on a text in the code without its German. Another language
  would get its own file next to them.
- **Wording in the UI:** plain, short, sentence case; say what happens ("Opens it in
  VS Code"), not how it's implemented.
- **Controls in the settings:** joined segments pick one of a few options; actions (Edit,
  Paste, Test, Open, Remove) are separate buttons with space between them, the card's main
  one primary (accent colour), the rest secondary, Edit with a pencil and Paste with a
  clipboard. A card with Edit has no Paste of its own: pasting is in the form Edit opens
  ("Paste address"). Remove, which forgets something saved, is red with a waste bin and asks
  first (`settings_window::destructive`). A choice of names (the language) is a drop-down
  list. Every page but About has a "?" after its title, to its `## <page> page` section of
  `docs/settings.md` (`help::PAGES`).
- **Extensions:** an optional feature is an extension: its switch is on the Extensions page,
  and only while it's on does its page show in the sidebar (`settings_window::page_shown`) or
  its part of the flyout. A new optional feature becomes one too. The user's own extensions
  (`extensions.rs`) are files Trayward only reads: never let one run, open or send anything (no
  links, commands, notifications, MQTT or push), and keep the format (`docs/extensions.md`,
  version 1) readable by every later version: new fields and row types are optional, and a row
  type a version doesn't know is skipped.
- **Status in the settings:** a card's description that says how something stands is a
  `settings_window::Status` with its tone: green works, blue under way (or worth knowing,
  nothing wrong: what a choice leaves out), yellow needs a look, red failed. A description too
  long for its card shows in full on hover.
- Short comments that explain _why_; small helpers. When a change is a design
  decision, record it where its topic lives (here or in `docs/`).
- **Plans and open items** go in `ROADMAP.md` (Next, Later, To investigate); take an item out
  when it's done (the changelog has it then).

## Testing

All tests run without the network (a server on 127.0.0.1 stands in where one is needed) and
without Claude Code.

- **Rust** (`cargo test`): parsing (session lists, transcripts, hooks, VS Code state),
  statistics, forecasts, time formatting, settings logic, the demo's transcripts. New
  logic that doesn't need a window gets a test, with a small sample line for every
  file format. The windows' layout and drawing are tested too, without a window:
  `render(None, …)` lays out, and `gfx::testing::offscreen` draws into memory. Coverage
  is measured in CI (cargo-llvm-cov, coverage.py) and aims at 80 % (SonarQube Cloud;
  what it leaves out, and why, is in `sonar-project.properties`).
- **Fuzzing** (`fuzz/`, cargo-fuzz): random bytes into the core's parsers (the agents'
  files, the usage endpoint's answer, the state file, addresses) through
  `trayward_core::fuzzing`; no input may panic. The workflow "Fuzzing" runs every target
  on pull requests that touch the core and longer every Sunday. A new parser of outside
  input gets an entry point there (and its target and seeds in `fuzz/`).
- **Release scripts** (`scripts/tests/`, stdlib `unittest`): version bumps, the
  changelog generator, the release notes, the pull request title help.
- **Visible changes:** run `--demo` and look, in light and dark mode, at 100 % and
  150 % scaling if layout changed. Smart App Control may block a fresh test binary
  (os error 4551): build again with a different optimisation level
  (`$env:CARGO_PROFILE_DEV_OPT_LEVEL = 1`).
- **Screenshots for the docs** (`docs/images/`) come from `--demo` only.

## Known limitations

- Windows 11 only, for now. There's no supported API for taskbar widgets: the widget is a
  child window of Explorer's taskbar. The taskbar can be at any edge (Windows 11 has
  allowed top, left and right since its 2026 updates); on the left or right the widget
  sits above the tray, two lines high on a wide taskbar when it fits, else stacked
  (`widget::column`), and popups open from the taskbar's edge
  (`taskbars::popup`).
- The agents' files and Claude's usage endpoint are undocumented; an update of an
  agent can break parts until Trayward catches up.
- The exe isn't code-signed yet: SmartScreen warns, Smart App Control may block it.
- Switching to a session in VS Code needs Claude Code's extension there (a session in a
  terminal is switched to by its window or tab).

## More detail

| File                                                       | What's in it                                                                                                                        |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| [docs/extensions.md](docs/extensions.md)                   | The format of the user's own extensions (a contract, version 1)                                                                     |
| [docs/how-it-works.md](docs/how-it-works.md)               | Where every number comes from: limits, sessions, hooks, VS Code, statistics, forecast, GPG, sounds, taskbars; the source map        |
| [docs/maintenance.md](docs/maintenance.md)                 | Versions, the changelog generator, the release flow and why it's built so, CI, Dependabot, SonarQube Cloud, rulesets, tool settings |
| [ROADMAP.md](ROADMAP.md)                                   | What's next, what waits for later, what still needs a look on a real PC                                                             |
| [.github/workflows/README.md](.github/workflows/README.md) | Each workflow, and what to do when it fails                                                                                         |

Read the parts that touch your change first: a change to what Trayward reads starts
with `docs/how-it-works.md`, one to the release flow with `docs/maintenance.md`.
