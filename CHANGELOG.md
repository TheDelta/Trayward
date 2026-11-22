# Changelog

All notable changes to Trayward. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/):

- **Major** (2.0.0): something you rely on changed: a setting goes away or works
  differently, the command-line flags, the hooks Trayward installs, or where it keeps
  its data.
- **Minor** (1.1.0): new features, nothing breaks.
- **Patch** (1.0.1): fixes.

The lines come from the pull requests: **Actions → Prepare release** adds one for
every pull request merged since the last release, with its author and the issues
it fixes, and opens a pull request to release them (see CONTRIBUTING.md). Each
release's notes on GitHub are its section here.

## [Unreleased]

<!--
  Prepare release adds a line here for every pull request merged since the last
  release, from its title (and from "* fix: …" lines in its squash description).
  Edit anything: a line that mentions a pull request (#12) replaces the ones that
  would be generated for it.

  - Highlights or an intro: a paragraph right here, above the first heading.
  - Breaking changes: a "⚠️ Breaking changes" section (feat!: and fix!: pull
    requests land there by themselves). Say what users have to do, e.g. which
    setting was renamed to what.
  - Upgrade notes, known issues: a "📝 Notes" section.
  - Sections: 🚀 Features, 🐛 Fixes, ⚡ Performance, 🌐 Translations,
    🔒 Security, 📚 Documentation, 📦 Dependencies, 🧰 Maintenance.
-->

## [0.1.0] - 2026-11-22

The first public release: your coding agents' usage limits and sessions in the taskbar.
Tested on Windows 11 with Claude Code. OpenAI's Codex CLI and GitHub Copilot's agent are
supported too, but untested so far: feedback is welcome. Linux, macOS and a signed exe are on
the roadmap. Built in the open: about 450 tests (about 91 % coverage), CodeQL, SonarQube Cloud
and OpenSSF Scorecard, and the exe built by GitHub Actions with a provenance attestation.

### 🚀 Features

- **Limits in the taskbar:** the weekly and 5-hour limits next to the clock, with the
  plan name on plans without a 5-hour limit, and an optional simulated 5-hour limit.
- **On every taskbar you want:** the main one, all of them, or the taskbars of the
  displays you choose; at any edge of the screen (at the left or right, above the tray: two
  lines on a taskbar with labels, a column on one without).
- **Room of its own:** optionally on Windows' Widgets button, which app buttons keep
  away from, wherever Windows puts it. What doesn't fit there gives way step by step (the
  usage bars, then the system monitor a column at a time, then slim), and the settings say
  what's left out.
- **Slim mode:** a narrower widget, its limits taking turns; today's limit takes turns
  with the 5-hour one in the normal widget.
- **Flyout:** reset times, the weekly limit split into days from midnight, each with its
  budget (what a day leaves goes to the days after it), even pace and where your pace
  leads. Pointing at a day shows what it used against its budget, what it saved, and how
  the week stands after it ("Week after this day: 3.0% under pace").
- **Fable's own limit:** on plans that give Fable a share of the weekly limit, its use
  and reset under Weekly; on by default.
- **Every session:** Claude Code sessions on Windows and in WSL with status, title, VS
  Code window, model, context size, tokens, running agents, the prompt cache timer,
  archived sessions, and background work that is still running (👀 Watching). A click
  opens the session in VS Code.
- **Instant updates:** optional hooks for immediate status changes and agent counts.
- **Notifications:** bubbles and synthesised sounds when a session needs you or is
  done, or your own WAV files; warnings when a limit gets close or your pace would use
  up the week early, at levels you choose, and when a limit is free again. Quiet while
  you present, play full screen, have Do not disturb on or snoozed them.
- **Countdown:** a nearly used-up limit shows the time until it resets.
- **Weekly summary:** a bubble on Monday morning about last week, and every week listed in
  the statistics, saved once it's over.
- **MQTT and Home Assistant:** limits (each agent's), sessions and events on your
  broker; Home Assistant finds the entities by itself (MQTT discovery). Over TCP or TLS
  (`mqtts://`); the broker typed in a form (host, port, user name, password) or pasted
  as an address. Sensors for sessions needing you, working and done, and for prompt caches
  about to expire; a limit your plan doesn't have (the 5-hour one on Ultra) gets no
  entity. Off by default.
- **Kept statistics:** sessions' numbers stay after Claude Code deletes their
  transcripts (Settings → Sessions).
- **Update check:** once a day, GitHub's newest release; a bubble and a menu entry when
  there's a newer one. Switch it off in Settings → About.
- **About page** in the settings: version, who made Trayward, the source code.
- **Away mode:** while you're away from the PC, sessions that need you go to your phone.
  Pick the service and fill in what it needs: ntfy (server, topic, access token), Gotify
  (server and app token), a Discord or Slack webhook, or any address that takes JSON; or
  paste an address. `http://` too, for a server on your own network. Kept encrypted;
  Remove forgets it. Off by default.
- **Merge requests and pipelines:** optionally, each session's merge or pull request
  and its pipeline, your own open ones in the flyout, and bubbles when a pipeline ends
  or a request is merged (in the flyout, as bubbles, or both); through `glab` and `gh`,
  with their own login or a read-only token you give Trayward (kept in Windows Credential
  Manager). The settings show whether the tools are found and logged in, and when a token
  runs out.
- **Codex:** OpenAI's Codex CLI as a second provider: its limits next to Claude's,
  its sessions in the list, read from its own files.
- **Copilot:** GitHub Copilot's agent (the CLI and the agent in VS Code): its sessions
  and statistics, and optionally its monthly limit through `gh`.
- **System monitor:** CPU, memory, GPU, network and disk next to the limits, with small
  graphs; off by default.
- **Session mailbox:** for sessions that leave each other messages in a shared folder:
  what's unread and handled, in the flyout; the folder is remembered once found. Off by
  default.
- **Your own extensions:** programs of your own write a JSON file, and Trayward shows its
  rows (a progress bar, a time, a text) in the tooltip and the flyout. Only read: nothing
  in it is run, opened or sent. Read when the file changes; each extension switched on
  one by one. Off by default; the format is in `docs/extensions.md`.
- **Terminal sessions:** a click brings the terminal to the front, in Windows
  Terminal on the session's tab.
- **Pin a session** to the taskbar, with its state or cache timer.
- **Tooltip** on the widget with the limits, their reset times and the sessions.
- **Without Claude Code:** on a PC with only Codex or Copilot, Trayward shows theirs and
  asks Anthropic for nothing.
- **Your plan, in style:** each plan's name has its own look; Ultra a badge that
  shines while you look at it.
- **Statistics:** today, 7 days, 30 days or all time, and per session: prompts, tokens,
  API value, turns, when you work, tools, commands, common failures, files and
  storage; searchable, sortable lists, CSV export, and the overview compared with the
  period before. A session's page copies the command that resumes it. Per project too,
  with which models wrote how much (share and tokens). With kept statistics, they open
  without reading every transcript again.
- **GPG key timer:** how long git can still sign commits without asking, and in the settings
  whether git signs with GnuPG and the key is found; off by default.
- **Settings window** in the style of Windows 11, light and dark, in English and German,
  with a **?** on every setting and every page: a short help on hover, the full help on
  click; drop-down lists, buttons and text fields like Windows' own, and status lines in
  colour. Optional features are extensions: switched on on the Extensions page, each adds
  its page (system monitor, away mode, MQTT, mailbox, your own extensions) or its part of
  the flyout (merge and pull requests, the GPG key timer).
- **Demo mode** (`--demo`): made-up sessions, usage and statistics, for screenshots
  and trying it out, as a Max or an Ultra plan.

[Unreleased]: https://github.com/TheDelta/trayward/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/TheDelta/trayward/releases/tag/v0.1.0
