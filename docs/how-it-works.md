# How Trayward works

Where every number comes from, and what Trayward does to get it. Everything described here happens
on your PC; the network requests go to `api.anthropic.com` (only when Claude Code is used on the PC),
once a day to `api.github.com` for the update check, and only if you switch them on to GitLab and
GitHub (merge requests and Copilot's monthly limit, through `glab` and `gh`), your push service (away
mode) and your MQTT broker.

- [Limits](#limits)
- [Sessions](#sessions)
- [Instant updates (hooks)](#instant-updates-hooks)
- [Switching to a session in VS Code](#switching-to-a-session-in-vs-code)
- [Switching to a session in a terminal](#switching-to-a-session-in-a-terminal)
- [Statistics](#statistics)
- [Weekly budget and pace](#weekly-budget-and-pace)
- [System monitor](#system-monitor)
- [GPG key timer](#gpg-key-timer)
- [Sounds](#sounds)
- [Codex](#codex)
- [Copilot](#copilot)
- [Session mailbox](#session-mailbox)
- [Your own extensions](#your-own-extensions)
- [Merge requests and pipelines](#merge-requests-and-pipelines)
- [Away mode](#away-mode)
- [MQTT and Home Assistant](#mqtt-and-home-assistant)
- [Update check](#update-check)
- [Taskbar placement](#taskbar-placement)
- [Source code map](#source-code-map)

## Limits

The numbers come from `GET https://api.anthropic.com/api/oauth/usage`, the endpoint behind Claude
Code's `/usage`, and the plan name from `/api/oauth/profile`. They are not public APIs and may
change.

**Fable's own limit** (Settings → Providers → **Fable limit**, on by default) comes from the same
answer: its `limits` list has an entry of the kind `weekly_scoped` whose scope is the model "Fable",
with the percentage used of Fable's own limit, and `weekly_scoped_shares` says how much of the weekly
limit that is ("up to 50 %", `allowance_percent_of_weekly`). Plans without it have no such entry,
and nothing shows. The flyout shows it under Weekly, the tooltip as a "Fable" row.

**Without Claude Code** (no `~/.claude` folder, or none at `CLAUDE_CONFIG_DIR`) nothing is
asked: no request goes to Anthropic, no error shows, and the flyout, the widget and MQTT leave
Claude's limits out (the widget shows Codex's or Copilot's instead). The folder is looked for again
with each refresh.

Trayward only **reads** Claude Code's login (`~/.claude/.credentials.json`) and never writes it.
Claude Code renews the login while you use it. When the saved login has expired, Trayward keeps the
last numbers (a limit that has reset since counts as 0 %) and catches up once Claude Code has
renewed it. Usage is fetched every 2 minutes by default (1 to 15 in the settings); the refresh
button rests 15 seconds after each use because the endpoint is rate-limited.

## Sessions

Every 2 seconds (5 or 10 in the settings) a background thread reads the list of running sessions
Claude Code keeps in `~/.claude/sessions/<pid>.json`: session id, folder, and whether it is busy,
waiting for you or idle. Sessions in WSL are read the same way from
`\\wsl.localhost\<distro>\home\<user>\.claude`, but only for distributions that are already running
(`wsl --list --running`), since opening a stopped one would start it; whether their process still
runs is checked in `/proc`. New distributions are looked for every 15 seconds, and `wsl` is asked
before every reading of one, so a distribution you stop isn't started again by the next one. When
`wsl` can't be asked, nothing in WSL is read that time. A distribution with sessions is read every
time; one without is only looked into every 30 seconds: WSL stops a distribution about 15 seconds
after its last process ended, and looking into its files counts as use, so read every 2 seconds an
unused one would never stop. The first session in a distribution that had none can therefore take
up to 30 seconds to show.

The transcripts (`~/.claude/projects/<folder>/<session>.jsonl`) give the title (the `ai-title` /
`custom-title` entries, as in the VS Code tab), the model, the context size, the tokens written and
when the prompt cache expires (the last request plus the cache lifetime it asked for,
`promptCacheRequested`: 5 minutes or 1 hour). They are read incrementally: only new lines. The VS
Code window name is the fixed start of `window.title` in the workspace's `.vscode/settings.json` (or
your user settings), else the folder name.

**Background work (👀).** A session can finish its turn but leave something running that will wake
it up again: a command started with `run_in_background`, a `Monitor`, a background agent, or a
`ScheduleWakeup`. The transcript shows when each starts ("Command running in background with ID
…", "Monitor started (task …, expires in 30m …)") and when it ends (a `<task-notification>` with a
status, or `TaskStop`), so the flyout lists what is still running under the session. Monitors and
wake-ups end at their time at the latest; work started before the session's Claude Code process
(from an earlier run) doesn't count.

**Running agents.** A badge before a session's state counts its subagents, whatever the state: a
turn can end ("Done") while background agents keep working. The count is the larger of what the
hooks say (`SubagentStart` / `SubagentStop`; a `SessionStart` drops agents of an earlier process)
and what the transcript says (`Agent` calls without a result yet, plus background agents still
running). Every `Agent` call can run in the background: forks (`subagent_type: "fork"`) do without
`run_in_background` in their input, so the call's result decides ("Async agent launched … agentId:
…"); a background agent ends with a `<task-notification>` for that id. A hook event sent while
Trayward isn't running is lost, so an agent whose `SubagentStop` never arrived would count forever:
the transcript corrects that. An agent it shows ended (a notification with a status for its id, or
a result naming its `agentId`) is dropped from the hooks' count.

**Archived sessions.** Sessions you archived in the Claude Code extension are left out, and come
back (faded, with an "archived" tag) only when they need you: an archived session still runs, and
messages from other sessions or its background tasks make it work on its own. With **Hide archived
sessions** off they're always listed, faded. The extension keeps the archived ids in VS Code's
global state (`hiddenSessionIds` in `%APPDATA%\Code\User\globalStorage\state.vscdb`), which
Trayward reads read-only with the SQLite library that ships with Windows (`winsqlite3.dll`); a
session that starts later takes the last ids read, so it doesn't show for a moment first.

**Hidden sessions.** The **×** on a session hides it until you write in it (a prompt of your own, from
the `UserPromptSubmit` hook or the transcript) or it needs you. Turns that don't come from you don't
count: messages from other sessions (`origin.kind: "peer"`, `promptSource: "system"` in the
transcript, "Another Claude session sent a message" as the prompt), task notifications and system
notes. Hidden ids are kept for 30 days after their session's process ends, so a resumed session
stays hidden. "Show N hidden sessions" at the bottom of the flyout brings all of them back.

**Seen.** A finished session stays "Done" until you look at it: by clicking it in the flyout or on
its bubble, by right-clicking its bubble, by hiding it, or
by having its VS Code window in front for 2 seconds after it finished. Trayward checks the window in
front once a second (`GetForegroundWindow` and its title, which Windows keeps without asking VS
Code). A VS Code title starts with the window's name (or names the folder, in VS Code's default
title), and names the active editor: when that is the session's tab, the title has its
conversation title. So a window marks the session whose conversation its title names, or else the
only finished one in that window; with several and none named, it marks none, since which tab you
looked at isn't known. Sessions in a terminal are only marked by a click (or a right-click on
their bubble). So "Done" means: finished,
and you haven't looked yet.

**Context getting full.** The context's share of the model's window: 1M tokens from the 4.6
models on (and `[1m]` variants), 200k before. Claude Code compacts the conversation automatically
about 33k tokens before the window is full (seen in `compact_boundary` entries: at about 967k of
1M). From three quarters of the way there, a session's row says "Context 80% full"; from 90% it
turns red: "compacts soon".

**Long lists.** The flyout shows the first 12 sessions (8 of them active); **+N more** shows all of
them and **Show fewer** goes back. When the flyout wouldn't fit on the screen, the list scrolls with
the mouse wheel.

All of these files are internal to Claude Code and undocumented, so Trayward reads them defensively
and skips anything unexpected. Sessions whose process has exited disappear. Without hooks, a
session that goes from busy to idle counts as done.

## Instant updates (hooks)

Optional. Turning on Settings → Sessions → **Instant updates** adds
[hooks](https://code.claude.com/docs/en/hooks) to `~/.claude/settings.json` that run
`trayward.exe trayward-hook`:

| Hook                                         | Effect                             |
| -------------------------------------------- | ---------------------------------- |
| `SessionStart`                               | Session appears (idle)             |
| `UserPromptSubmit`                           | Working                            |
| `Notification` (permission prompt, question) | Needs you                          |
| `PostToolUse`, `PostToolUseFailure` (async)  | Back to working after you answered |
| `Stop`                                       | Done                               |
| `SubagentStart`, `SubagentStop`              | Running agents are counted         |
| `SessionEnd`                                 | Session disappears                 |

A tool event doesn't say whose it was. When one arrives while a session needs you, it may come
from other work running in parallel (a subagent, another tool call) with the prompt still open. So
if Claude Code's session list still says "waiting" a few seconds later, the session needs you
again, since when it first did.

The hook prints nothing and always exits 0, so it never changes what Claude does. It sends a small
message to the running app (`WM_COPYDATA`) and is gone in a few milliseconds. With hooks, a turn
you interrupt with Esc (no `Stop` hook fires) shows as idle instead of done.

Turning the hooks off removes exactly these entries and nothing else. If the hooks name another
exe than the one that runs (it moved, or another copy runs), Trayward updates its own entries on
the next start. Sessions that were already open when the hooks were
added may only pick them up after a restart; until then the session list covers them.

## Switching to a session in VS Code

1. Claude Code's VS Code extension lists each window's workspace folders in `~/.claude/ide/*.lock`.
   The folder containing the session's working directory identifies the window. The extension
   sometimes loses these files while its windows stay open (seen when one window updates the
   extension and the others keep the old one). Without a lock, Trayward looks for the working
   directory, or the nearest folder above it, among the folders VS Code records for its windows
   (`windowsState` in its `storage.json`), and needs an open window whose title names it too. The
   window name is the folder name or the fixed start of `window.title`, as for the session list.
   Neither alone is enough: a title can name another folder of the same name, and the record can
   still hold a window just closed. Nothing found: the switch says so, rather than opening a new
   window.
2. Starting VS Code with that folder focuses the window that has it open. For WSL sessions it
   passes the window's `vscode-remote://wsl+<distro>/<folder>` URI instead.
3. `vscode://anthropic.claude-code/open?session=<id>` then makes the extension reveal the session's
   tab. The first time, VS Code asks whether the extension may open the URI; tick **Don't ask again
   for this extension**.

Step 3 opens a second copy of a session in whichever VS Code window is active, so it only runs once
the right window is in front. Multi-root workspaces can't be targeted this way and show a message
instead.

## Switching to a session in a terminal

A session started from a terminal on Windows has the Claude process's id in its session list.
That process's console is attached to for a moment (`AttachConsole`, by a helper: see below) to
find its window:

- **The classic console:** the console window itself comes to the front.
- **Windows Terminal:** the console is a pseudo console whose hidden window is owned by the
  terminal's window, which comes to the front. To pick the tab, Trayward gives the console a unique
  title for a moment, finds the tab showing it with UI Automation, selects it, and puts the old
  title back (within a fraction of a second; the tab's name may flicker).
- **A terminal inside VS Code** (or Cursor, Windsurf, VSCodium): its pseudo console belongs to no
  window. Trayward tells it by VS Code among the Claude process's parents (the shell, VS Code's
  terminal host, VS Code). It gives the console a unique title the same way. VS Code shows that
  title in two places UI Automation can see: the terminal's text area ("Terminal 3, `<title>`"),
  only while that terminal is shown, and its entry in the terminal list beside the panel ("Terminal 3
  `<title>`"), when the panel has more than one terminal. Trayward looks for either, first in the
  window named after the session's folder, then in all of VS Code's windows; that window comes to
  the front, a list entry is selected, and the terminal gets the focus.
  - **What it found is remembered** (the window and the terminal's number, while Trayward runs): when
    the title can't be seen next time (the terminal is hidden behind another one in a panel with a
    single list entry, or the panel is closed), the window it was in comes to the front.
  - **Otherwise** the window named after the session's folder comes to the front, without picking
    the terminal.

Sessions in a terminal in WSL can't be switched to: their process ids belong to Linux.

What happens to a console happens to every process attached to it: when it closes (its tab or
window is closed), Windows ends them all, and gives a process no clean way to stay. So Trayward
doesn't attach itself. For the second or two of a switch it starts itself once more as a helper
(`trayward.exe trayward-terminal <pid> …`, see `terminal.rs`), which attaches, finds the window,
selects the tab or shows the terminal, writes one line saying what it found and ends. If the
terminal closes right then, only the helper ends with it, and the switch says the session has
ended. A Ctrl+C or Ctrl+Break in that console meanwhile is ignored by the helper. The argument is
Trayward's own and can change with any version.

## Statistics

The statistics window reads the session transcripts (Windows, and WSL when enabled). Every chart
shows its values on hover, tiles with a › open a list, and pages scroll. Claude Code sometimes
writes earlier lines of a transcript again later in the file: each reply (by its message id), tool
call (by its id) and prompt (by its line's `uuid`) counts once.

Days, weeks and the hours of the "when you work" map are local time, by the offset that applied at
each moment: over a range with a clock change, a day still starts at its own midnight (and is 23 or
25 hours long once).

- **Overview** for today, the last 7 or 30 days, or all time: sessions, projects and active hours;
  prompts, replies, tool calls, agents started, tokens written (subagents included), the share of
  input served from the prompt cache, failed tool calls, files changed, lines written and replaced,
  commits and web lookups; API value; turns, time Claude worked, median and longest turn; tokens
  written per hour, day or week; the weekly limit over its window with earlier weeks in grey;
  when you work (prompts and replies by weekday and hour); projects (each opens its page), tools,
  shell commands and models (share and tokens written, subagents included) ranked; the sessions that wrote the most; the longest turns; common failures.
- **Projects:** every project (a session's folder, whichever agent worked in it) with its sessions,
  prompts, tokens written, API value, failed calls and when it was first and last used, the latest
  first, or sorted by any of these or by name. A project's page is the overview of its sessions for the chosen period, with its sessions
  where the projects are and, where the weekly limit is (the limit isn't a project's), its models:
  each one's share of the tokens written, tokens written and read (input and cache), replies and
  API value, subagents included. Its numbers don't open the lists of all sessions' calls and files.
- **Weekly summary:** from Monday 08:00 local time, once per week, a bubble with last week's
  prompts, tokens written, API value and time Claude worked (Monday 00:00 to Monday 00:00, the
  same numbers as the overview for that range). It waits while it's quiet and isn't shown for a
  week without prompts; the week it was made for is kept in `state.json` (`summary_week`). A click
  opens the weeks.
- **Weeks:** every week with its sessions, prompts, tokens written, API value and the time Claude
  worked, the latest first, with a bar for the tokens against the week that had the most. This
  week is counted as far as it is. A week that is over is saved in `state.json` (`weeks`, up to
  520 of them) when the summary is made or the page first shows it, so it stays as it was even
  when its transcripts and kept statistics are gone. A later count replaces what was saved only
  when it found more (a WSL distribution that wasn't running the first time). A week starts at
  its own Monday midnight, also when the clocks changed during it.
- **Starting context** (overview and per session): how big the context was at the first reply,
  before any work, and what makes it up. See below.
- **Agent map** (per session): the session and the subagents it started, as a tree; hover an agent
  for its type, model, tokens, tool calls, API value and how long it ran. Claude Code keeps each
  subagent's transcript in `<session>/subagents/agent-<id>.jsonl`, with `agent-<id>.meta.json`
  next to it (`description`, `agentType`, `finished` once it is done). An agent counts as running
  while it isn't finished and its transcript was written in the last 10 minutes. A session's
  tokens written and API value include its subagents; its prompts, replies, tool calls, files and
  turns are the main conversation's only, so each agent's own numbers are in the map. The map
  shows the first 12 agents; **Show all** shows the rest, and the page scrolls.
- **Per session:** the same numbers for one session plus how long it ran and its main model; the
  context size over time (compactions show as drops) and its peak against the model's window;
  its models (each one's share of the tokens written, tokens written and read, replies and API
  value, subagents included) and its agents summed up (how many it started and how many still
  run, their types and models, how long they ran in total and the longest, what they wrote and
  its API value; an agent runs from its first reply to its last, and agents side by side each
  count);
  tokens written per hour; the token mix;
  tools; the files changed most; and the disk space the session takes (transcript, subagent
  transcripts, saved tool output, file history), with buttons that open the project and the
  transcript folder (Explorer is only shown a folder that exists on a drive or in WSL, never
  another PC's share).
- **Tool calls:** every call with its time, tool and what it worked on (command, file, pattern,
  URL); failed ones with their error, calls you declined or a permission check blocked in amber.
  Filter by failed, by tool or by kind of failure; hover a row for the full text. A long list
  shows its first 2,000 rows (search or filter to narrow it down); the CSV has all of them.
- **Files:** every file read or changed, with edits, lines written and replaced, reads and when.
  A call that failed, was declined or was blocked counts as no change and no read.
- **All sessions:** every transcript, including past sessions and ones archived in the extension.
- **Storage:** the size of each of Claude Code's folders and of each project's transcripts.

Main numbers of the overview are compared with as long a time right before (today with yesterday
until the same time, 7 days with the 7 days before, 30 with 30): ▲ or ▼ and the change, more on
hover.

Lists can be searched (just type; Backspace deletes, Esc clears, Ctrl+V pastes), sorted (click the
selected sort again to reverse it) and exported: **CSV** saves what the page lists, search applied,
to your Downloads folder, with commas between the columns whatever the PC's region (a spreadsheet
that expects `;` imports it as comma-separated).

**Resume command** on a session's page copies `cd <folder>; claude --resume <id>`
(`copilot --resume <id>` for a Copilot session), which
continues the session in a terminal: PowerShell for a Windows session, the shell in the
distribution for a WSL one. A folder with spaces or other special characters is put in single
quotes, so nothing in its name is expanded or run when the command is pasted.

**API value** prices every reply with Anthropic's API prices for its model: input, output, cache
reads, and cache writes at 1.25× input for 5 minutes or 2× for an hour (the transcript says which).
It shows what the usage is worth, not what a subscription costs. Prices of older models are
approximate.

**Turns** run from a prompt to the last reply or tool call before the next prompt, or to the first
pause of 30 minutes (later activity was set off by something else, such as a background task
reporting back). They include time Claude waited for your answers. Summaries that continue a
conversation after compaction are not prompts.

**Starting context and setup.** Before the first prompt, Claude Code writes what it loads into the
transcript as `attachment` entries: `skill_listing` (skills, with a count and names),
`deferred_tools_delta` (tool names; MCP tools are `mcp__<server>__<tool>`),
`mcp_instructions_delta` (MCP servers' instructions), `agent_listing_delta` (agent types),
`instructions` (CLAUDE.md, AGENTS.md and rules files; `AutoMem` is auto memory's index, MEMORY.md)
and `prompt_snapshot` (the system prompt). The starting context is the first reply's context
(exact). Its parts are estimates: each listed part's characters divided by four. The rest, the
first reply's context minus those, is mostly the built-in tools' definitions, which the transcript
doesn't list; in practice it is the largest part (about 35k of 47k). The **Setup** card counts
skills, MCP servers and their tools, agent types and instruction files, and whether the memory
index was loaded; the counts follow later changes in the session (an MCP server that connects
late). Memory files are counted on disk: the `.md` files in `projects/<folder>/memory` other than
the index, for the session's project, or for all projects on the overview, whose card shows the
latest session's setup. Older transcripts have none of this and say so.

**Common failures** group failed tool calls by the program or tool and the first telling line of the
error, with numbers and paths left out.

When a project moved, Claude Code keeps a copy of its transcripts in the old project folder; only
the latest copy of each session counts.

**Performance.** Tool results make transcripts large, so only failed ones are parsed; the rest are
skipped by a quick text check. Transcripts are read in 4 MB chunks on a low-priority background
thread; each file is read once and later only what was appended. Ten minutes after the window is
closed everything read is released again, and a restart has nothing in memory either. So that the
next time doesn't start over, a session's kept statistics (below) say how far its transcript was
read: reading goes on from there, and only what was written since is read. On about 3 GB of
transcripts the first "all time" view ever takes 5 to 10 seconds, with kept statistics later ones
under a second, and reopening it while it's in memory about 100 ms. Without kept statistics every
first view after a pause reads everything again. While it reads, the page says how many sessions
are done. Folder sizes are cached for 5 minutes.

**Kept statistics.** Claude Code deletes transcripts after `cleanupPeriodDays` (30 days by default),
and their statistics would go with them. With Settings → Sessions → **Keep statistics** on (the
default), a minute after start and then every 6 hours Trayward saves the numbers of every session
that changed (and hasn't changed for 5 minutes) to `%LOCALAPPDATA%\Trayward\kept\<windows or
wsl-distro>\<session id>.json`: the session's title and folder, the prompts' starts, replies with
their tokens and models (no text), tool calls with what they worked on (the command, file, pattern or
address, up to 200 characters) and their error (up to 300), and the subagents' replies (tokens and
models). So a kept file holds parts of what you asked and ran, not the conversation itself: no
reply text and no tool output. Once a
transcript is gone, the session counts from its kept file everywhere in the statistics; its page
says "transcript deleted, statistics kept" and has no resume command. Switching the option off
deletes the folder.

A kept file also says where reading stopped, so that it can go on there (see Performance):

- **How far the transcript was read** (`offset`, in bytes), a check of the 256 bytes before that
  (`mark`), what was already counted (ids as numbers, since Claude Code writes lines again), and
  each subagent transcript's name, time and size. Reading goes on from a kept file only when the
  bytes before `offset` are still the same; a subagent's transcript that changed is read again.
- **Its time** is when the session was last written (its transcript or a subagent's), so an
  unchanged session isn't saved again. The statistics window writes a kept file too: the first time
  it reads a session, and again once 4 MB of its transcripts are new.
- **Which version counted it** (`parser`, the number `stats::PARSER`). When a version counts
  transcripts differently (a fix, or something new it reads), that number goes up, and every kept
  file with another number is made again from its transcript. A session whose transcript is
  already gone keeps the numbers it was last counted with.

A WSL distribution that isn't running can't be read without starting it. Its sessions count from
what was kept of them, and show like ones whose transcript is gone, until it runs again.

## Weekly budget and pace

The weekly bar in the flyout is split into the seven days of the limit's window, with today
outlined. Days run from local midnight (`forecast::day_spans`); when the limit resets at another
time, the shorter of the window's first and last pieces joins the day next to it, so there are
always seven days of 12 to 36 hours, named by the calendar. Settings → Limits → **Weekly bar**
chooses how they look:

- **Days** (the default): seven equal parts, each filled by what its day used of its budget, green
  within it and yellow over it (red on a week that runs out); today is yellow too once it's ahead of
  its pace. A soft fill and a small marker show how much of today has passed.
- **Budgets**: the bar is the weekly limit, filled to its percentage. Each day is as wide as its
  budget (the days before today as wide as what they used); the soft fill and the marker show where
  today's budget, spread over the day, would be now.

The flyout's title, "Usage", opens claude.ai's usage page in the browser. That page also shows
what the usage endpoint doesn't report, such as the limit resets that are available: the endpoint
Trayward reads has no field for them (checked 2026-11-15), so the flyout doesn't show them.

**Its colour says how the week goes** (`forecast::Plan::standing`), for the bar and the percentage:

- **Green:** at or under an even pace (up to 1 % over it counts as on pace).
- **Yellow:** over an even pace: slow down. Also when today is over its own pace
  (`forecast::Plan::today_over_pace`: more than today's budget spread over its hours so far, again
  with 1 % to spare), so a week that's on pace still warns on a day that goes too fast; and from
  80 % used, whatever the pace.
- **Red:** this week's own pace would reach more than 110 % by the reset, so the limit runs out
  well before it. Also from 95 % used.

In the taskbar, the weekly limit (`W`) turns yellow and red the same way; on pace it keeps its usual
colour. Above the bar, with one decimal:

- **Today**, next to "Weekly": what today used ("(Today: 5.2%)"), in yellow from 90 % of today's
  budget or over its pace, and in red over its budget.
- **Even pace**, on the right: the percentage next to where an even spread would be now
  ("14.5% of 15% even pace").

Below the bar, after the reset time, the **prediction**: this week's average use per day (what it
used since it began, or since a change of plan, over the time since), where it leads (about how
much will be used at the reset, or when 100 % is reached, in amber, if that comes first) and the
past weeks' pace to compare with: "Prediction: 7.3%/day · ~55% at the reset (past weeks: ~6%/day)".
In the week's first hours it goes by the past weeks instead, and says so. The pace warning goes by
the same.

**Each day's budget** (Settings → Limits → **Daily budget**), by the day's hours. _Adaptive_, the
default: a day may use what was left of the week when it began, spread over it and the days after
it (on the first day, its hours' share of the week), so what a day leaves unused goes to the days
after it and what it uses beyond its budget comes off them. Days still to come get today's budget
(by their hours) until today ends: they change once a day, not with every reading. _Even_: a
seventh of the week, every day. What a day began with comes from the readings Trayward keeps (the last one before the
day began); when there's none, from the day's first reading, so a day Trayward started in counts
from then.
Pointing at a day shows its name, with when it runs if that isn't midnight to midnight ("Sat
(02:00 – Sun 00:00)"); its budget; what it used (from the readings before and after it) and how that
compares, in one line: "Used 12.0% (1.1% under its budget)", green within its budget, red over it;
today's with what's left for today, in yellow when it's over its pace, with how far ("Over pace:
1.3% more than even by now"). Then what the days so far saved or overspent together: how far the
week was under or over an even spread when that day ended ("Week after this day: 3.0% under pace";
today: "Week so far", by now), from the week's percentage then (`forecast::Plan::week_after`). A day
over its budget that the days before had saved enough for adds ", evened out". With adaptive
budgets the days' own differences don't quite add up to it: what a day saves already raises the
budgets after it. A day that used more than its budget has its name in
yellow; today also gets a yellow outline, and on a week that's on pace its part of the bar turns
yellow. Today over its pace looks the same. The tooltip on the taskbar says "over pace" in its
Today row and shows the Weekly row in yellow.

**Today's limit** (`D` in the widget) is what today used as a share of today's budget: over 100 %
it's yellow, and over today's pace too. It takes turns with the 5-hour limit (or the plan's name on Ultra) in the widget's
second row, every 10 seconds by default (Settings → Taskbar → **Take turns**: off, 5, 10, 20 or
30 seconds). The widget is as wide as the wider of the two, so the turns don't move it. The tooltip
always has a "Today" row: "2.1 % of 14 % daily budget", in yellow with how much is over once
today used more than its budget.

**A change of plan.** The limit is a percentage of the plan's limit, so after an upgrade the same use
is a smaller percentage, after a downgrade a bigger one. Each reading keeps the plan it was taken
on, and readings of another plan, or from before the percentage fell within its window, aren't
compared with today's: today's use and budget count from the first reading on the new plan, the
days before it show no use, and the pace leaves the jump out. The plan's name is asked for every
few hours; a fall of the percentage, or a rise of more than 10 points between two readings, asks
for it with the next refresh.

The usage endpoint only reports the current percentage, so Trayward keeps its own history: a sample
at most every 15 minutes when it changes, kept for eight weeks in `state.json`. This week's pace
appears once six hours of it have passed, the past weeks' once a few hours of them were recorded.

**Limit warnings.** With bubbles on, a bubble warns once per limit window when the weekly limit
reaches 80 % and 95 %, when the 5-hour limit reaches 90 %, and when your pace would use up the
weekly limit before it resets. Clicking it opens the flyout. Which warnings were shown is kept in
`state.json`, so a restart doesn't repeat them.

**Simulated 5-hour limit** (Settings → General, off by default): on plans without a 5-hour limit,
Trayward simulates one from the same history. Like the real limit, a window starts with the first
use after the previous one ended and lasts five hours; its "limit" is a day's share of the weekly
limit (100 % ÷ 7).

## System monitor

Optional, off by default (Settings → Extensions → System monitor, one switch for all of it, which adds
its page; switched on the first time, it starts with CPU and memory): CPU, memory, GPU, network and disk use in the widget, each
switched on its own (switching the monitor off keeps which ones), network and disk in a left column, CPU, memory and GPU in a
right one, with a faint graph of the last 30 seconds behind each value. Read on a background thread
once a second while anything is on, and nothing at all while everything is off:

- **CPU:** `GetSystemTimes`, busy time against all time since the last second.
- **Memory:** `GlobalMemoryStatusEx`, physical memory in use: as a percentage or in GB (1024³
  bytes, as Task Manager counts them).
- **Network:** `GetIfTable2`, the byte counters of hardware adapters that are up; their filter
  copies and virtual adapters (Hyper-V, WSL) are left out, or traffic would count twice. Bytes per
  second, up ↑ and down ↓; the graph is scaled to the busiest second shown.
- **Disk:** the performance counter `\PhysicalDisk(_Total)\% Idle Time`; busy is the rest, like
  "active time" in Task Manager. Or the read (R) and write (W) rates per second
  (`Disk Read Bytes/sec`, `Disk Write Bytes/sec`), in a column of their own.
- **GPU:** the performance counters `\GPU Engine(*)\Utilization Percentage`, one per process and
  engine: summed per engine, the busiest engine, as Task Manager shows it. Or the video memory in
  use, in GB: `\GPU Adapter Memory(*)\Dedicated Usage` of the graphics card with the most memory.
  The cards and their sizes come from DXGI, which only lists them: no device is created, so no
  graphics driver is loaded into Trayward.

The widget's tooltip shows every value switched on in detail, whichever way the widget shows it:
memory and video memory as used, size and percentage, the disk as busy time, read and write rates,
network sent and received. It updates every second while it's open, and keeps its size: it grows at
once, but shrinks only when its content has been clearly smaller (more than a few pixels) for 3
seconds, so rates that change every second don't make it jump.

Values keep a fixed width so the widget doesn't move every second; from 90 % a percentage (or
memory in GB that full) turns amber, from 95 % red.
The demo shows made-up values.

## GPG key timer

If git signs commits with GPG (`commit.gpgsign`), a thin line along the bottom of the widget and a
row in the flyout show whether the signing key is unlocked and about how long it stays so. gpg-agent only says
whether the passphrase is cached (`KEYINFO`), not for how long, so Trayward estimates it from the
agent's settings (`default-cache-ttl`, restarted by every signature, and `max-cache-ttl`, counted
from unlocking), when it saw the key get unlocked, and the last signed commit in the repositories of
your sessions: the newest commit, amend, merge, cherry-pick or rebased commit in `.git/logs/HEAD`
(checkouts, resets and fast-forwards sign nothing). When it saw the key get unlocked is kept in
`state.json` with the agent's process id, so a restart of Trayward doesn't lose it; a new agent has
forgotten the passphrase anyway. A key that was already unlocked when Trayward first looked, with nothing
saved, can't have been unlocked before the agent started: it says "stays so for at least …" counted
from then. Signatures it can't see (another repository, a tag) can make the key
stay unlocked past the estimate; then it says "for now".

Trayward only asks: it uses the GnuPG git is set up with, asks the agent every 30 seconds
(`gpg-connect-agent --no-autostart`, so it never starts an agent), and stops any of GnuPG's tools that
take longer than 5 seconds, so a stuck agent can't hold up the rest of the app. It never signs and
never asks for the passphrase.

The setup is looked up in this order, and the card in Settings → Extensions names the first step
that fails: git in `PATH`; `commit.gpgsign` true and `gpg.format` unset or `openpgp`; GnuPG's tools
next to git's `gpg.program` or in `Program Files\GnuPG\bin`; the signing key's keygrip
(`gpg --list-keys` for `user.signingkey`, else the first secret key that can sign); the agent's
answer to `KEYINFO`. The setup is looked up again every 10 minutes, the agent asked every 30
seconds. While the timer is off, nothing is run: git and GnuPG are only looked for, in `PATH` and
`Program Files`, every 10 minutes.

## Sounds

The built-in sounds are synthesised (no audio files): soft bell, marimba and wood tones, a few sine
partials each with a short fade-in and a natural decay, about a second long and quiet. "Done" sounds
resolve (a rising fifth, a marimba landing on the home note, a major arpeggio); "needs you" sounds
stay open (two knocks, a single ping, a small unresolved step up). Custom sounds must be WAV files.
Sounds and bubbles skip the first seconds after start-up, so sessions catching up don't announce
themselves.

**Quiet times.** No sounds or bubbles while Windows says the user is busy (presenting, a full-screen
app or game: `SHQueryUserNotificationState`) or has **Do not disturb** on. Windows has no documented
API for Do not disturb; Trayward reads the quiet hours profile the shell publishes
(`NtQueryWnfStateData`, `WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED`: 0 off, otherwise on), as other
apps do. Limit warnings aren't dropped meanwhile: they come with the next reading after the quiet
time. The levels they come at are set in Settings → Limits.

## Codex

When `~/.codex/sessions` exists (or `CODEX_HOME` points elsewhere), Trayward also reads OpenAI's
Codex CLI, every 3 seconds on its own thread, and only its files: no sign-in, no network.

- **Files:** Codex writes one JSON-lines file per session, `sessions/YYYY/MM/DD/rollout-*.jsonl`.
  Only days whose folder changed in the last 9 days are looked at (every 10 minutes all of them,
  and from then on the ones with a session still in use: a session that goes on for longer stays
  in its first day's folder), and of each file only the first
  64 KB (the session's id and folder, the first prompt) and the last 256 KB (the latest state):
  these files can grow to hundreds of megabytes. A file is read again only when it changed.
- **Limits:** the newest `token_count` record with `rate_limits` across those files: `primary` and
  `secondary`, each with `used_percent`, `window_minutes` (a window of up to 10 hours is the
  5-hour one) and `resets_at` (older versions: `resets_in_seconds` after the record). The plan comes
  from `plan_type`. They're as of the last time Codex was used; once a window's reset time has
  passed, it counts as unused.
- **Sessions:** files changed within a day. Written to in the last 15 seconds: working. A tool
  call without a result for 20 seconds: needs you (Codex doesn't record that it waits for an
  approval, so this is a guess). A reply after the last prompt: done, idle after an hour.
- **In the taskbar:** a second block with Codex's limits, each block with a small mark (✻ Claude,
  ⬡ Codex; plain symbols, not logos). Settings → Providers picks All, Claude or Codex.

Codex sessions can't be switched to: their files don't say which terminal runs them.

## Copilot

When `~/.copilot/session-state` exists (or `COPILOT_HOME` points elsewhere), Trayward also reads
the sessions of GitHub Copilot's agent, every 3 seconds on its own thread, and only its files: no
sign-in, no network. The Copilot CLI and the agent in VS Code write the same files.

- **Files:** a folder per session, `session-state/<id>/`, with `events.jsonl` (one event per line,
  appended as it happens; only what was added is read, and event types Trayward doesn't use are
  skipped without parsing), `workspace.yaml` (the folder and the session's name, which is the
  title) and `inuse.<pid>.lock` for the process that runs it.
- **Which sessions:** those whose lock names a running process and whose events changed within a
  day. Locks of ended processes stay behind, and a pid can be reused, so the process must have
  started before the lock was written. A session that was shut down (`session.shutdown`) isn't
  listed until it's resumed.
- **State:** a turn runs from `assistant.turn_start` to `assistant.turn_end` or `abort`: working.
  While the tool `ask_user` has no result, or a `permission.requested` has no
  `permission.completed`: needs you. A turn that ended: done (after 2 quiet seconds, since an
  answer can take several turns), idle after an hour. Unlike Codex, nothing is guessed from a tool
  that takes long.
- **Model and context:** from `session.usage_record` (the request's input tokens, cached parts
  included), before the first one from `session.auto_mode_resolved` or `session.model_change`.
- **The monthly limit** isn't in the files: Copilot keeps its quota readings in memory. With
  Settings → Providers → Copilot → **With limit**, Trayward runs
  `gh api copilot_internal/user` every 5 minutes (and on **Refresh now**), with gh's own login
  (never Trayward's token for github.com, which is made for pull requests). After a failure it asks again a minute later, then
  waiting twice as long each time, up to half an hour. The flyout's "As of" is when GitHub last
  answered. From the answer's
  `quota_snapshots` it takes the premium requests when the plan has them, else the free plan's
  chat messages; a plan with neither limited shows none. The endpoint is undocumented. When
  asking fails, the last numbers stay and the settings card says why.
- **Where it shows:** a section in the flyout (what's counted, how many are left, the reset), a
  row in the tooltip, `copilot_monthly` in the MQTT state, and a block in the taskbar with the
  mark ◈: `M` with the used percentage, below it how many are left. With Codex on, the block
  shows when the taskbar shows everyone's limits (Settings → Providers → In the taskbar: All).
  In slim mode the limit joins the turns.

With the limit shown, a bubble warns when it reaches the weekly limit's warning level
(Settings → Limits) and again at 95 %, once a month each; one that had to wait (right after
start, Do not disturb, snoozed) comes once it may. In the taskbar's dots, Codex's and Copilot's
sessions count while they need you or work (not when done: nothing says you've seen them), and
the needs-you dot glows for them too; the MQTT state counts them the same way.

**Statistics.** Copilot's sessions are in the statistics like Claude Code's: the overview, the
list of all sessions (marked "Copilot"), a session's page, tool calls, files and the CSV. Their
`events.jsonl` is read into the same numbers: a prompt per `user.message`, a reply per
`session.usage_record` with its tokens (Copilot's input count includes the cached part, which is
taken out) and model, a tool call per `tool.execution_start`, failed when its
`tool.execution_complete` says so. Their API value is $0: Copilot's models aren't priced per
token here. They aren't kept after their files are deleted (Keep statistics is for Claude Code's
transcripts), and with Copilot switched off they aren't read. In the flyout, a Copilot session
has the statistics button.

Copilot sessions can't be switched to yet.

## Session mailbox

Opt-in and off by default, for one particular setup: sessions that can't message each other (one in
Windows, one in WSL) leave Markdown messages in each other's inbox folders in a shared
`.claude-mailbox` folder. Trayward reads that folder every 10 seconds (only reads: it never moves or
writes a message) and shows what's unread for whom, how long it has waited and what was handled:
in the flyout, as `✉ 2` on the session whose inbox it is, and in the tooltip. The whole feature,
the folder's layout and how it's found: [mailbox.md](mailbox.md).

## Your own extensions

Off by default (Settings → Extensions → Your custom extensions). Programs of the user's write one JSON
file each into `%LOCALAPPDATA%\Trayward\extensions\`; Trayward reads them and shows their rows in
the tooltip and the flyout, each extension only once the user switched it on. The format and its
limits: [extensions.md](extensions.md).

**Only read.** A file can say which rows are shown and nothing else: no links, commands or
notifications, and nothing from it goes to MQTT or away mode. Text is cleaned of control
characters and invisible direction marks and cut to its length; a file over 64 KB, a link to
somewhere else and more than five files aren't read. A file that can't be read (not JSON, a field
wrong) is listed in the settings with the reason, never stops Trayward, and shows nothing.

**When it's read.** Windows tells Trayward when a file in the folder changes (a change
notification on the folder: its thread waits without using the CPU); a moment later, when the
writer is done, the folder is read, and only the files whose time or size changed are read again.
Besides, it looks once a minute, as a change can go untold. A time row is counted on by Trayward
every second, so a program only writes when something changed. Switched off, nothing is read and
the folder isn't watched. The reading is in `core/src/extensions.rs`; the folder watch in
`core/src/os/` (other systems look every few seconds instead).

## Merge requests and pipelines

Off until switched on (Settings → Extensions), because it contacts GitLab and GitHub. Trayward
doesn't talk to them itself: it runs their command-line tools, `glab` and `gh`.

- **Which login:** Trayward's own token for the host, when the user pasted one (Settings → GitLab
  and GitHub), else the tool's own login. The token is kept in Windows Credential Manager, a
  generic credential named `trayward:gitlab:<host>` or `trayward:github:<host>`, which Windows
  encrypts with the user's login (DPAPI). Trayward reads it each time it runs the tool and hands it
  over only as an environment variable of that one process: `GITLAB_TOKEN` and `GITLAB_HOST` for
  `glab`, `GH_TOKEN` for `gh` on github.com (`GH_ENTERPRISE_TOKEN` and `GH_HOST` on another host).
  Never on the command line, which any program of the user can read; never in `state.json`, the
  registry, a log or a message (anything shaped like a GitLab or GitHub token is replaced by
  `[token]` in errors). Pasting takes it from the clipboard and empties the clipboard; the demo
  keeps none. In the core: `forge.rs` (`run_tool`, `save_token`), `os/windows/secret.rs`.
- **When a token runs out:** asked after a login check that worked: GitLab's
  `personal_access_tokens/self` (`expires_at`), GitHub's `github-authentication-token-expiration`
  header (`gh api --include user`). The host's card turns yellow 14 days before.

- **Which branch:** a session's folder (or a folder above it) has a `.git`: its `origin` remote
  names the host and project (`git@host:group/project.git`, `https://host/group/project`), `HEAD`
  the branch. In a `git worktree`, `.git` is a file pointing to the worktree's own folder; the
  remote comes from the main repository's config. github.com is GitHub. Hosts known to be something else (Bitbucket, Codeberg, Azure DevOps, Gitea.com,
  SourceHut and a few more: `forge::not_gitlab`) are never asked. Another host is asked
  as GitLab only when `glab` has a login for it: `glab auth status --hostname <host>`, once per
  host and login, remembered (a host without a login is checked again after half an hour, since
  glab can take half a minute to give up on it; one with a login after 12 hours). Repositories on any other host are left alone.
- **How the tools stand** (the card in Settings → Extensions): whether `glab` and `gh` are found
  (looked up in `PATH`, as an `.exe`, `.cmd` or `.bat`: the `PATH` Trayward started with, then the
  one the registry has now, so a tool installed since is found without restarting Trayward), and for each host the sessions' projects
  are on whether the tool has a working login there (`glab auth status --hostname <host>`,
  `gh auth status --hostname <host>`, by their exit code; github.com is always asked of `gh`,
  because Copilot's monthly limit is read there). A login is asked for once and remembered, a
  missing one again after half an hour. While merge and pull requests are switched off, neither
  tool is run: only whether they are found is shown.
- **One request per host, every minute** (and when the flyout opens): a GraphQL query through
  `glab api graphql --hostname <host>` or `gh api graphql` asks for the newest merge or pull
  request of each session's branch with its pipeline (or checks), the branch's own latest
  pipeline (GitLab), and the user's own open requests. Of those, only the ones in a project a
  session works in are kept: a host without a session isn't asked.
- **Shown:** a pill under the session (`!37 · Pipeline running`, `main · Pipeline passed`), with the request's title cut to about 24 characters when it says something the session's own title doesn't (`!37 Fix login · Pipeline running`), and
  "Your MRs and PRs" in the flyout, the first six: failed pipelines first, then running ones, then the most
  recently updated. A click opens the page in the browser (https links only).
- **Bubbles:** when a pipeline that was running passes, fails or is canceled, or an open request is
  merged. They show the GitLab or GitHub logo (drawn from their vector marks, `logo.rs`) instead of
  the session dot, the number and title, and the state as a pill: green for passed or merged, amber
  for canceled, red for failed, the same colour along the left edge.

## Away mode

Off until switched on (Settings → Extensions → Away mode, which adds the Away page), because it sends
to an address outside the PC.

- **Away** means no mouse or keyboard input for the chosen time (5 to 30 minutes,
  `GetLastInputInfo`). A locked PC gets there the same way.
- **What goes out:** when a session (Claude Code, Codex or Copilot) starts needing the user, or
  finishes if "Done too" is on, the bubble's headline: "shop needs you", "shop is done". No
  prompts, replies or paths. Snoozing stops pushes; presenting and Do not disturb don't (they're
  about this PC).
- **Where:** a service picked in the settings (**Edit**) with what it needs, or taken from a
  pasted address (`push::Config::from_address`), kept in the registry as JSON, encrypted for this
  Windows account (`PushAddress`; an address an earlier version saved there, or as plain text in
  `PushUrl`, is turned into a setup the first time it's read). `https://`, or `http://` for a server on
  the user's own network: not encrypted, so the settings show it in yellow with a warning. The
  settings show the service and its host, never a topic, token or webhook address: those are
  secrets. Per service:
  - **ntfy:** JSON publishing, `{"topic", "title", "message"}` to the server's root (or its path),
    with `Authorization: Bearer <token>` when there's an access token.
  - **Gotify:** `{"title", "message", "priority": 5}` to `<server>/message`, the app's token in the
    `X-Gotify-Key` header (not the query: addresses end up in servers' logs).
  - **Discord** `{"username", "content"}`, **Slack** `{"text"}`, **Webhook**
    `{"title", "message", "source"}`, each to its address.
- **Remove** forgets the setup.
- **Test** sends "Trayward can reach your phone." and shows whether the server took it. The
  demo never sends.

## MQTT and Home Assistant

Off until switched on (Settings → Extensions → MQTT, which adds the MQTT page), because it connects
to a broker. Trayward has its own small
MQTT 3.1.1 client (no extra library): TCP (`mqtt://`, port 1883 by default) or TLS (`mqtts://`, port
8883 by default) through Windows' own SChannel (`os::tls`, TLS 1.2). Windows checks the broker's
certificate: its chain against the certificate store and its name against the host, so a
self-signed certificate works once its CA is in Windows' trusted root certificates. QoS 0, a clean session, a ping every 30 seconds, and after a failure a new try after
5, 10, 30, 60 and then every 300 seconds. The broker is typed into the settings in parts (protocol, host, port, user name, password) or
pasted as one address; the address with the user name is kept in the registry (`MqttBroker`), the password encrypted with DPAPI for the Windows account
(`MqttPassword`). The demo never connects.

With `<node>` the PC's name in lowercase (the client's name comes from it too, so two PCs of the same
name, or two accounts on one PC, take each other's place on one broker: that's expected, name them
differently):

| Topic                                                    | Retained | What                                                                                              |
| -------------------------------------------------------- | -------- | ------------------------------------------------------------------------------------------------- |
| `trayward/<node>/availability`                           | yes      | `online`; `offline` when Trayward exits or as its last will                                       |
| `trayward/<node>/state`                                  | yes      | JSON, sent when it changes                                                                        |
| `trayward/<node>/event`                                  | no       | JSON `{"event_type", "session", "text"}`                                                          |
| `homeassistant/<component>/trayward_<node>/<key>/config` | yes      | Home Assistant discovery, one per entity: on each connect, and when a provider is shown or hidden |

The state:

```json
{
  "claude_plan": "Max",
  "claude_weekly": 46,
  "claude_weekly_resets": "2026-11-15T02:00:00Z",
  "claude_five_hour": 31,
  "claude_five_hour_resets": "2026-11-12T21:00:00Z",
  "claude_fable": 4,
  "claude_fable_resets": "2026-11-15T02:00:00Z",
  "codex_plan": "Plus",
  "codex_weekly": 41,
  "codex_weekly_resets": "2026-11-16T13:52:00Z",
  "codex_five_hour": 18,
  "codex_five_hour_resets": "2026-11-12T22:26:00Z",
  "copilot_plan": "Pro",
  "copilot_monthly": 25,
  "copilot_monthly_resets": "2026-12-01T00:00:00Z",
  "needs_you": 1,
  "working": 2,
  "done": 1,
  "caches_expiring": 1,
  "caches_expiring_sessions": [{ "name": "api", "expires": "2026-11-12T20:41:00Z" }],
  "sessions": [{ "name": "shop", "state": "working", "provider": "claude" }]
}
```

Each provider's keys start with its name; a provider's values are `null` while Trayward doesn't
show it (switched off, or not used on this PC), and so are limits the plan doesn't have (Claude's
5-hour limit on Ultra unless it's simulated, Fable's on plans without one or while it's switched
off). The sessions are every provider's, each with its `provider`. `needs_you`, `working` and `done`
count as the widget's dots do. `caches_expiring` counts the sessions that need you or have finished whose prompt
cache runs out within 10 minutes (when the warning bubble comes); `caches_expiring_sessions` says
which, and when each runs out.

Event types: `needs_you` and `done` (a session of any provider, whether or not a bubble shows),
`limit_warning` and `free_again`. In Home Assistant the device "Trayward `<node>`" (with Trayward's
version and a link to its repository) gets, per provider, sensors for its limits (%), when they
reset (timestamps) and its plan: Claude's while Claude Code is used on the PC, Codex's and Copilot's while Trayward shows them
(their entities are removed when it stops). A limit the plan doesn't have gets no entity (it would
always be unknown): Claude's 5-hour limit on Ultra, unless it's simulated, and Fable's while there's
none or it's switched off. Then sessions needing you, working and done (with the session list as an
attribute), caches expiring soon (with those sessions and when their caches run out as an
attribute), a "Needs you" binary sensor and a "Session" event entity. Entities of earlier
versions, whose keys didn't name a provider (`weekly`, `plan`, …), are removed. Only names and states
go out: workspace names, no prompts, replies, titles or paths.

## Update check

Once a day (and when "Check now" in Settings → About is clicked), `GET
api.github.com/repos/TheDelta/trayward/releases/latest`, without a login. A newer version
(`v1.2.3`; drafts and pre-releases don't count) shows in Settings → About and the right-click menu,
and a bubble tells about it once per version; a click opens its release page (only pages of the
project on GitHub). Nothing is downloaded or installed. No release yet (404) counts as up to date.
When it last asked and which version a bubble already told about are kept in `state.json`. The
demo never asks.

## Taskbar placement

Windows 11 has no supported way to add things to the taskbar. The widget is a small transparent
window placed inside the taskbar, left of the tray area, and re-attached when Explorer restarts.
With many apps pinned, left-aligned icons or labelled buttons, taskbar buttons can run underneath
the widget. Trayward finds where the buttons end with UI Automation (every few seconds, on its own
thread) to decide whether the usage bars fit; **Solid background** hides what still runs underneath.

**On the Widgets button.** Windows has no API to keep room on the taskbar, but it keeps the app
buttons away from its own Widgets button (the weather). Settings → Taskbar → **Position: Widgets**
puts the widget there: UI Automation finds the button (automation id `WidgetsButton`) on each
taskbar, wherever Windows puts it (the left end with centred icons, next to the tray with
left-aligned ones). A widget narrower than the button takes all of it, its content centred and the
button covered in the taskbar's colour (as with Solid background), so the weather doesn't show
beside it and the whole button is the widget's to point at. When it's wider than the button, it leaves
out the usage bars (even when they're always wanted), then the system monitor's columns one at a
time from the left (network and disk before CPU, memory and GPU), and then goes slim by itself
(`widget::fit`); the settings' Position and System monitor cards say what was left out, with the
button's width and what all of it would need. If it's still wider, it reaches from the button
towards the middle of the taskbar. Without a Widgets button (Widgets switched off in Windows'
taskbar settings) it sits next to the tray, and the settings card says how to switch Widgets on.

**Taskbar at the top, left or right.** Windows 11 can put the taskbar at any edge. Which one is
worked out from the taskbar's shape and place on its display. At the top the widget is the same
as at the bottom. At the left or right it sits just above the tray area (on other displays'
taskbars, above the clock, found by UI Automation). On a taskbar wide enough (one with labels) it's
the widget as at the bottom, two lines high and centred, without bars, plan name or pinned session.
When all of it doesn't fit there (the system monitor with many values), it's stacked in parts that
are each laid out as at the bottom where they fit: the monitor's columns side by side (merged into
two if needed), below them the session dots next to the limits. Otherwise it's a column as wide as
the taskbar: the system monitor's
values, the session dots a line per state, then every limit on a line of its own, label left and
value right. Its lines are inset like the tray's icons (a tenth of the width, 5 to 16 px a side)
and at most 132 px wide, so a label and its value stay together on a taskbar with labels. On a
narrow one, a part (the monitor's values, the limits) whose lines don't all fit side by side is
stacked as a whole, the label above the value, centred like the clock. No usage bars, no
plan name, no pinned session, and no turns: there's room for all limits at once. **Position:
Widgets** doesn't apply there. The flyout, bubbles and tooltip open from the taskbar's edge: the
one the taskbar takes from the work area, or, when it hides itself, the screen edge nearest to the
widget.

**Slim.** Settings → Taskbar → **Display mode: Slim** (or the right-click menu) makes the widget narrower: the session dots in a row on top,
and below one limit at a time, taking turns as often as Settings → Taskbar → **Take turns** says
(weekly, the 5-hour limit, today's limit, then Codex's and Copilot's when they're shown). A plan
without a 5-hour limit (Ultra) has the weekly and today's limit. With turns off, it shows the
weekly limit. The slim
widget leaves out the plan's name, a pinned session and the system monitor; the tooltip still shows
everything. It's as wide as its widest limit, so the turns don't move it, and the clock picks the
turn, so widgets on several taskbars show the same limit.

**Several displays.** With "Show my taskbar on all displays", each other display has a taskbar of
its own (`Shell_SecondaryTrayWnd`). Settings → Taskbar → **Show on taskbars** puts a widget on the
main taskbar, on all of them, or on the taskbars of chosen displays. These taskbars have no tray
window, so the widget sits left of their clock, which UI Automation finds too (it waits for that
before showing; without a clock, left of the show-desktop strip). Displays are named by their
model (from Windows' display configuration) and remembered by the monitor's device path, which
stays the same across restarts as long as it's plugged into the same port. A chosen display that
isn't connected is skipped; if none is, the main taskbar shows the widget, so it never disappears.
The flyout, statistics and settings open next to the widget you clicked last; bubbles appear on the
display of the window in front.

The widget is a layered (per-pixel transparent) window drawn in software. The flyout, the bubble and
the settings and statistics windows draw with Direct2D in software too, on purpose: a GPU render
target loads the graphics driver into the process, which on an NVIDIA card took about 70 MB more
memory and 35 more threads, far more than a few small windows need. Each window's render target is
released once it's hidden and made again when it shows. While it waits, Trayward uses about 15 MB
(Task Manager's memory column).

## Source code map

Two crates: the Windows app (`src/`: the widget, the windows, the settings) and the core
(`core/src/`, the crate `trayward-core`: the logic without the UI). Everything the core needs from
the system goes through its `os` module, so the core can serve an app on another system; see
AGENTS.md, "Providers and platforms".

The app:

| File                     | Contents                                                                             |
| ------------------------ | ------------------------------------------------------------------------------------ |
| `src/main.rs`            | Entry point: the app, the hook (`trayward-hook`) or the terminal helper              |
| `src/app.rs`             | App state, event queue, timers, window procedures                                    |
| `src/widget.rs`          | The widget, one per taskbar it's shown on, and its placement                         |
| `src/taskbars.rs`        | The taskbars, the displays they're on, and which ones show the widget                |
| `src/flyout.rs`          | The flyout with limits and sessions                                                  |
| `src/menu.rs`            | Right-click menu                                                                     |
| `src/settings_window.rs` | The settings window                                                                  |
| `src/edit.rs`            | The settings' text fields: Windows' edit controls over the drawn boxes               |
| `src/help.rs`            | The settings' "?": short help on hover, its section of docs/settings.md on click     |
| `src/i18n.rs`            | The app's German translations, on top of the core's                                  |
| `src/hits.rs`            | Clickable areas of the flyout, the settings and the statistics window                |
| `src/stats_window.rs`    | The statistics window: pages, charts with tooltips, lists                            |
| `src/sysmon.rs`          | The optional system monitor: CPU, memory, GPU, network and disk use                  |
| `src/gfx.rs`             | Direct2D and DirectWrite drawing                                                     |
| `src/theme.rs`           | Light and dark colours                                                               |
| `src/collector.rs`       | Background thread: session lists, transcripts, window names                          |
| `src/uia.rs`             | Where the taskbar buttons end and the clocks of other taskbars start (UI Automation) |
| `src/bubble.rs`          | The "done" / "needs you" bubble                                                      |
| `src/tip.rs`             | The widget's tooltip                                                                 |
| `src/plan.rs`            | How each plan's name looks                                                           |
| `src/logo.rs`            | The GitLab and GitHub logos for their bubbles                                        |
| `src/terminal.rs`        | Switching to a session in a terminal; the helper that attaches to its console        |
| `src/sound.rs`           | Built-in sounds and playback                                                         |
| `src/vscode.rs`          | VS Code windows: names, switching to a session                                       |
| `src/vscode_state.rs`    | Archived sessions, from VS Code's global state                                       |
| `src/settings.rs`        | Preferences in the registry, "Start with Windows"                                    |
| `src/win.rs`             | Small Win32 helpers: window messages, windows, clipboard, the hook's delivery        |

The core, in `core/src/`:

| File             | Contents                                                                         |
| ---------------- | -------------------------------------------------------------------------------- |
| `lib.rs`         | What the core is: the logic without the UI                                       |
| `os/mod.rs`      | What it asks of the system; the clock, running and finding programs              |
| `os/windows/`    | Windows: processes, local time, language, folder changes, HTTPS, TLS (SChannel)  |
| `os/other.rs`    | Other systems, for now: fallbacks so the core builds and its tests run           |
| `i18n.rs`        | English and German: the language, the core's translations                        |
| `stats.rs`       | Statistics from transcripts, prices, disk usage                                  |
| `setup.rs`       | What a session loads: skills, MCP servers, instructions, memory, and their sizes |
| `keep.rs`        | Kept statistics: sessions' numbers after Claude Code deletes their transcripts   |
| `usage.rs`       | Reading the login, fetching usage                                                |
| `sessions.rs`    | Session states and how the session list and hook events change them              |
| `registry.rs`    | Claude Code's session lists on Windows and in WSL                                |
| `transcripts.rs` | Incremental transcript reading: title, model, tokens, agents, background work    |
| `jsonl.rs`       | Append-only JSON-lines files read as they grow (transcripts, Copilot's events)   |
| `codex.rs`       | Codex: limits and sessions from its files                                        |
| `copilot.rs`     | Copilot: sessions from its files, the monthly limit through gh                   |
| `mailbox.rs`     | The session mailbox (opt-in): finding and reading its folder                     |
| `extensions.rs`  | The user's own extensions (opt-in): their JSON files, read when they change      |
| `forge.rs`       | Merge and pull requests and pipelines, via glab and gh                           |
| `push.rs`        | Away mode's pushes: the services, their setup, the request per service           |
| `update.rs`      | The update check: GitHub's latest release, compared with this version            |
| `mqtt.rs`        | MQTT: the client, Home Assistant discovery, reconnecting                         |
| `alerts.rs`      | When limit warnings, "free again", the countdown and the weekly summary come     |
| `gpg.rs`         | The GPG signing key's cache                                                      |
| `forecast.rs`    | Weekly budget per day, pace, simulated 5-hour limit                              |
| `hook.rs`        | The hook mode Claude Code runs (the app delivers its message)                    |
| `tracking.rs`    | Adding and removing the hooks in Claude Code's settings                          |
| `store.rs`       | Saved state (and moving it over from the app's earlier name)                     |
| `timefmt.rs`     | Time formatting                                                                  |
| `demo.rs`        | Demo mode: made-up sessions, transcripts, usage and history                      |
