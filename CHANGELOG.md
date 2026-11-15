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

The first public release: Claude Code's usage limits and sessions in the Windows 11
taskbar.

### 🚀 Features

- **Limits in the taskbar:** the weekly and 5-hour limits next to the clock, with the
  plan name on plans without a 5-hour limit, and an optional simulated 5-hour limit.
- **On every taskbar you want:** the main one, all of them, or the taskbars of the
  displays you choose.
- **Flyout:** reset times, the weekly limit split into days, even pace, what's left per
  day and where your pace leads.
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
- **Weekly summary:** a bubble on Monday morning about last week.
- **MQTT and Home Assistant:** limits, sessions and events on your broker; Home
  Assistant finds the entities by itself (MQTT discovery). Over TCP or TLS (`mqtts://`).
  Off by default.
- **Kept statistics:** sessions' numbers stay after Claude Code deletes their
  transcripts (Settings → Sessions).
- **Update check:** once a day, GitHub's newest release; a bubble and a menu entry when
  there's a newer one. Switch it off in Settings → About.
- **About page** in the settings: version, who made Trayward, the source code.
- **Away mode:** while you're away from the PC, sessions that need you go to your phone
  (ntfy, Discord, Slack or any JSON address); off by default.
- **Merge requests and pipelines:** optionally, each session's merge or pull request
  and its pipeline, your own open ones in the flyout, and bubbles when a pipeline ends
  or a request is merged; through `glab` and `gh`.
- **Codex:** OpenAI's Codex CLI as a second provider: its limits next to Claude's,
  its sessions in the list, read from its own files.
- **Terminal sessions:** a click brings the terminal to the front, in Windows
  Terminal on the session's tab.
- **Pin a session** to the taskbar, with its state or cache timer.
- **Tooltip** on the widget with both limits, their reset times and the sessions.
- **Your plan, in style:** each plan's name has its own look; Ultra a badge that
  shines while you look at it.
- **Statistics:** today, 7 days, 30 days or all time, and per session: prompts, tokens,
  API value, turns, when you work, tools, commands, common failures, files and
  storage; searchable, sortable lists, CSV export, and the overview compared with the
  period before. A session's page copies the command that resumes it.
- **GPG key timer:** how long git can still sign commits without asking.
- **Settings window** in the style of Windows 11, light and dark.
- **Demo mode** (`--demo`): made-up sessions, usage and statistics, for screenshots
  and trying it out, with every plan to choose from.
