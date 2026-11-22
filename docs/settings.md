# Trayward settings

What each setting in Trayward's settings window does. Open the window with a right-click on the
widget → **Settings…**, or with **⚙** in the flyout. The **?** after a setting's title brings you
to its section on this page, the one after a page's title to that page's part.

A change takes effect at once: there's no Save button (only the forms for an address have one).
A line that says how something stands is coloured: green when it works, blue while it's under way,
yellow when it needs a look, red when it failed. A line too long for its card shows in full when the
mouse is on it. The settings are kept for your Windows
account (in the registry, `HKCU\Software\Trayward`). Where a number comes from and what Trayward
does to get it: [How Trayward works](how-it-works.md).

- [General](#general-page): starting with Windows, how often usage is read, animations, language
- [Taskbar](#taskbar-page): where the widget sits and how it looks
- [Sessions](#sessions-page): how sessions are found and what's read about them
- [Notifications](#notifications-page): bubbles and sounds
- [Limits](#limits-page): warnings, the daily budget, the weekly bar and the countdown
- [Providers](#providers-page): the agents Trayward reads: Claude Code, Codex, Copilot
- [Extensions](#extensions-page): optional features: each adds its settings while it's on
- [System monitor](#system-monitor-page): your PC's load next to the limits
- [GitLab and GitHub](#gitlab-and-github-page): glab and gh, and Trayward's own tokens for them
- [Away](#away-page): notifications on your phone while you're away
- [MQTT](#mqtt-page): limits and sessions for Home Assistant
- [Mailbox](#mailbox-page): mail between sessions in a shared folder
- [Custom extensions](#custom-extensions-page): what your own programs report
- [About](#about-page): the update check

## General page

### Start with Windows

Starts Trayward when you sign in to Windows. Off until you switch it on. The entry names
`trayward.exe` where it is now: if you move the exe, start it once from its new place and the
entry follows. Switch it off before you delete the exe.

### Refresh usage every

How often Trayward asks Anthropic for your limits: **1**, **2** (the default), **5**, **10** or
**15** minutes. The usage service is rate-limited, so Trayward doesn't ask more often than this (apart from once
right after a limit resets), and waits longer when the service says so. **↻** in the flyout updates at once (then rests 15
seconds). It asks `api.anthropic.com` with the login Claude Code saved; on a PC without Claude
Code (no `~/.claude` folder) it asks nothing. See [Limits](how-it-works.md#limits).

### Animations

The glow of a session that needs you, the shake it gives now and then, and the spinner while usage
is refreshed. On by default. Switch it off for a calmer taskbar: colours and counts still say the
same.

### Simulate a 5-hour limit

For plans without a 5-hour limit: Trayward simulates one from your weekly use. A window starts
with your first use after the last one ended and lasts five hours; its limit is a day's share of
the week (100 % ÷ 7). Off by default. On plans with a real 5-hour limit it changes nothing.

### Language

- **System** (the default): Windows' display language, German if that's German, else English.
- **English** or **Deutsch**: always that language, whatever Windows uses.

## Taskbar page

### Position

Where the widget sits on the taskbar.

- **Tray** (the default): left of the tray icons; on other displays' taskbars, left of
  the clock. With many buttons or left-aligned icons, app buttons can run under the widget
  ([Usage bars](#usage-bars) and [Solid background](#solid-background) help with that).
- **Widgets**: over Windows' Widgets button (the weather on the taskbar), wherever Windows
  puts it: at the left end when the taskbar icons are centred, next to the tray when they're
  left-aligned. Windows keeps app buttons away from that button, so they no longer run under the
  widget. The widget fits itself to the button's width: narrower, it takes the whole button in the
  taskbar's colour so the weather doesn't show beside it; wider, it leaves out the usage bars (even
  with Usage bars on Always), then the system monitor's columns one by one (network and disk
  first, CPU, memory and GPU last), and goes slim if that's not enough. The card then says how
  wide the button is, what all of it would need, and what's left out; Tray shows all of it.

**Widgets** needs Widgets switched on: Windows Settings → Personalization → Taskbar →
**Widgets**. Without it, Trayward goes back next to the tray. Trayward covers the button, so the
Widgets panel no longer opens from that spot.

With the taskbar at the left or right (Windows Settings → Personalization → Taskbar → Taskbar
behaviors → Taskbar position), the widget sits just above the tray, whatever Position says,
without the usage bars. On a wide taskbar (one with labels) it's two lines as at the bottom, or,
with many system monitor values, the monitor above the session dots and limits. On a narrow one
it's a column: the session dots, then each limit on a line of its own.
The flyout and the bubbles open next to the taskbar, wherever it is.

### Display mode

- **Normal** (the default): two rows like the clock, the weekly limit on top and the 5-hour limit
  below, with the session dots and counts beside them.
- **Slim**: narrower. The session dots in a row on top, and one limit at a time below, taking
  turns (see [Take turns](#take-turns)). It leaves out the plan's name, a pinned session and the
  system monitor; the tooltip still shows everything.

Slim is also in the widget's right-click menu. Try it when the widget covers taskbar buttons.

### Take turns

How long each limit is shown before the next one's turn: **Off**, **5**, **10** (the default),
**20** or **30** seconds.

- **Normal display:** today's limit (`D`, what today used of its daily budget) takes turns with the
  5-hour limit in the second row. Off: the 5-hour limit stays.
- **Slim display:** all limits take turns: weekly, 5-hour, today's, then Codex's and Copilot's when
  they're shown. Off: the weekly limit stays.

The widget's tooltip always has today's row. More on today's limit:
[Weekly budget and pace](how-it-works.md#weekly-budget-and-pace).

### Usage bars

The small usage bars in the widget.

- **Automatic** (the default): only when there's room before the taskbar buttons.
- **Always** or **Never**.

### Solid background

Gives the widget a solid background, so taskbar buttons that run under it are covered instead of
showing through. Off by default. With [Position](#position): **Widgets**, buttons stay away
from the widget and you rarely need it.

### Show on taskbars

Which taskbars show the widget when you have several displays.

- **Main** (the default): the main taskbar only.
- **All**: every taskbar, including those of displays you connect later.
- **Choose**: a switch for each display appears below. A chosen display that isn't connected is
  skipped; when none is, the main taskbar shows the widget, so it never disappears.

Windows must show a taskbar on those displays: Windows Settings → Personalization → Taskbar →
Taskbar behaviors → **Show my taskbar on all displays**.

## Sessions page

### Instant updates

Adds hooks to Claude Code's settings (`~/.claude/settings.json`), so a session shows as working,
waiting for you or done the moment it happens, and running agents are counted as they start and
stop. Off by default.

Without them, Trayward reads Claude Code's session list (see
[Check sessions every](#check-sessions-every)), and a session that goes from busy to idle counts as
done. Switching them off removes exactly Trayward's entries and nothing else; it's the only change
Trayward ever makes to Claude Code's files. Sessions already open may only pick up the hooks after a
restart.

**Needs repair** means some of Trayward's entries are missing or point to another copy of the exe:
switch it on to fix them. Details: [Instant updates (hooks)](how-it-works.md#instant-updates-hooks).

### Session details

Reads each session's transcript for its title, model, context size, tokens written and the timer of
its prompt cache, and for background work it left running (👀) and a context that's getting full.
On by default. Only new lines are read. Off: the sessions are listed with less detail and no cache
timer.

### Sessions in WSL

Also lists Claude Code's sessions in WSL, in distributions that are already running (Trayward never
starts one, and leaves one without sessions alone long enough for WSL to stop it), and includes
them in the statistics. On by default. Switch it off if you don't use Claude Code in WSL. The first
session in a distribution can take up to 30 seconds to show. Sessions in a WSL terminal can't be
switched to; in VS Code they can.

### Keep statistics

Claude Code deletes transcripts after 30 days (its `cleanupPeriodDays`), and their statistics would
go with them. With this on (the default), Trayward keeps what the statistics show of them in
`%LOCALAPPDATA%\Trayward\kept`, so "all time" stays all time. That is more than numbers: a session's
title and folder, the start of each prompt, and every tool call with what it worked on (the command,
file or address, shortened) and its error, if it had one. Claude's replies and what the tools
returned aren't kept. It also makes the statistics open quickly: Trayward goes on from what was
kept and reads only what was written since, instead of every transcript again; and the sessions of
a WSL distribution that isn't running still count. **Switching it off deletes what was kept.**
Details: [Statistics](how-it-works.md#statistics).

### Hide archived sessions

Sessions you archived in Claude Code's VS Code extension are left out of the list. They come back,
faded and tagged "archived", while they need you: an archived session still runs. On by default.
Off: they're always listed, faded.

### Check sessions every

How often Trayward reads Claude Code's list of running sessions: **2** (the default), **5** or
**10** seconds. Less often uses less power, and a change shows a few seconds later. With
[Instant updates](#instant-updates) on, the hooks report most changes at once anyway.

## Notifications page

No bubbles or sounds while you present, play full screen, have Windows' **Do not disturb** on, or
snoozed them (right-click → **Snooze notifications**: 30 minutes, 1 hour, 2 hours or until
tomorrow morning, 08:00; the same menu ends it early).

### Bubble

A small bubble above the taskbar says which session is done or needs you; a click takes you there.
A right-click closes it where you are: a finished session counts as seen, one that needs you stays
so until you answer it.
On by default (also in the right-click menu, as **Notifications**). It's the switch for all of Trayward's bubbles: limit
warnings, "free again", the weekly summary, pipelines and new versions too.

### Sound when a session needs you

**Off**, **Knock** (the default), **Ping**, **Nudge**, or **Custom**: a WAV file of your own (the
folder button picks another one). The play button lets you hear it. It also plays when a prompt
cache is about to run out and when a pipeline fails.

### Sound when a session is done

**Off**, **Chime** (the default), **Marimba**, **Arpeggio**, or **Custom**: a WAV file of your own.
It also plays when a limit is free again, a pipeline passes or a request is merged.

### Pipeline bubbles

Shown while [Merge and pull requests](#merge-and-pull-requests) is on: a bubble when a pipeline
passes, fails or is canceled, or a request is merged. On by default. The same as **Bubbles** in
[Where it shows](#where-it-shows) on the GitLab and GitHub page.

### Volume

How loud the built-in sounds are: **Quiet**, **Medium** (the default) or **Loud**. A custom WAV file
plays as loud as it was recorded. Changing it plays the "done" sound once. **Mute sounds** in the
right-click menu silences all of them for now and keeps your choices.

## Limits page

Each warning is a bubble (so [Bubble](#bubble) must be on) and comes once per limit window. While
you present or have Do not disturb on, it waits and comes afterwards.

### Weekly limit

A bubble when the weekly limit reaches this level, and again at 95 %: **Off**, **70**, **80** (the
default) or **90 %**. With Copilot's monthly limit shown, it warns at the same levels. **Off** means
no warning at all: not at 95 % either, and none for Copilot's limit.

### 5-hour limit

A bubble when the 5-hour limit reaches this level: **Off**, **75**, **90** (the default) or
**95 %**.

### Daily budget

How the weekly limit is split into days: the days of the flyout's weekly bar and today's limit
(`D`) in the widget.

Days run from midnight to midnight. When the limit doesn't reset at midnight, the hours before the
first midnight or after the last one belong to the day next to them: with a reset on Saturday at
02:00, Saturday has 22 hours and Friday runs until the reset, 26 hours. A day's budget goes by its
hours.

- **Adaptive** (the default): a day may use what was left of the week when it began, spread over it
  and the days after it. What a day leaves unused goes to the days after it; what it uses beyond its
  budget comes off them. The days still to come get today's budget until today ends.
- **Even**: a seventh of the week, every day.

Details: [Weekly budget and pace](how-it-works.md#weekly-budget-and-pace).

### Weekly bar

How the flyout's weekly bar shows the days. For trying both; one of them may go again.

- **Days** (the default): seven equal parts, one per day, each filled by what its day used of its
  budget: green within it, yellow over it (today also when it's ahead of its pace). The soft fill
  shows how much of today has passed.
- **Budgets**: the bar is the weekly limit, filled to its percentage. Each day is as wide as its
  budget, the days before today as wide as what they used, so a day that used more makes the days
  after it narrower.

### Pace

A bubble when this week's pace would use up the weekly limit before it resets. On by default. The
pace is this week's average use per day; in the week's first hours it goes by the past weeks.

### Countdown

Once a limit is used up to this level, the widget shows the time until it resets instead of the
percentage: **Off**, **90 %**, **95 %** (the default) or **Full** (only once it's used up).

### Free again

A bubble, and the "done" sound, when a limit that was at 80 % or more resets. On by default.

### Weekly summary

On Monday morning (from 08:00), a bubble about the week before: prompts, tokens written, API value
and how long Claude worked. A click opens the statistics on **Weeks**, where every week is listed
with these numbers (they are saved once a week is over, with or without the bubble). It skips a
week without prompts. On by default.

## Providers page

### Claude Code

Always on: limits, sessions and statistics. There's nothing to switch; the card is there so you
see what Trayward reads. On a PC without Claude Code (no `~/.claude` folder) it says so, and
Trayward shows the other agents' limits without asking Anthropic for anything; once Claude Code is
used, it comes in by itself.

### Fable limit

On plans where Fable has a limit of its own (a share of the weekly limit, for example "up to 50 % of
your weekly limit"), the flyout shows it under **Weekly**: how much of Fable's own limit is used and
when it resets, as Claude Desktop shows it. The tooltip has a "Fable" row, and MQTT publishes it as
`claude_fable`. On by default; on plans without it, nothing shows.

### Codex

OpenAI's Codex CLI: its 5-hour and weekly limits next to Claude's, and its sessions in the same
list. On by default; it shows nothing until Codex is used on this PC (`~/.codex`). Trayward reads
Codex's own files: no sign-in, no network. Codex's limits are as of its last use, and its sessions
can't be switched to. Details: [Codex](how-it-works.md#codex).

### In the taskbar

Shown while [Codex](#codex) is on: whose limits the widget shows.

- **All** (the default): Claude's and Codex's, and Copilot's monthly limit when it's shown.
- **Claude** or **Codex**: only theirs.

### Copilot

GitHub Copilot's agent (the Copilot CLI, or the agent in VS Code).

- **Off**.
- **Sessions** (the default): its sessions in the list and the statistics, read from Copilot's own
  files (`~/.copilot`): no sign-in, no network.
- **With limit**: also how much of the month's premium requests (or, on the free plan, chat
  messages) is used, in the taskbar, the flyout and the tooltip. Trayward asks GitHub for it every
  5 minutes through the GitHub CLI, `gh`, with gh's own login: it must be installed and signed in
  (`gh auth login`). When asking fails, the card says why.

Details: [Copilot](how-it-works.md#copilot).

## Extensions page

Optional features, each with its switch here, all off until you switch them on. Switched on, an extension adds its page to the list on the left
or its part of the flyout; switched off, its page goes, and its settings stay for the next time. A
page's own switch does the same, and switching it off there brings you back here (Your custom extensions
is switched only here: its page lists the extensions found).

- **System monitor**, **Away mode**, **MQTT**, **Mailbox**, **Your custom extensions**: each adds its
  page, where its settings are explained: [System monitor](#system-monitor-page),
  [Away](#away-page), [MQTT](#mqtt-page), [Mailbox](#mailbox-page),
  [Custom extensions](#custom-extensions-page).
- **Merge and pull requests** and the **GPG key timer**: below. Merge and pull requests also add
  [Pipeline bubbles](#pipeline-bubbles) to the Notifications page.

### Merge and pull requests

Each session's branch shows its merge or pull request and the pipeline's state (e.g.
`!37 · Pipeline running`), and the flyout lists your own open merge and pull requests in the
projects your sessions work in, failed pipelines first. A click opens it in the browser. Off by
default, because it contacts GitLab and GitHub.

Trayward runs their command-line tools, `glab` and `gh`: with a token of its own for the host when
you pasted one on the [GitLab and GitHub page](#gitlab-and-github-page), else with the tool's own
login (`glab auth login`, `gh auth login`). It asks once a minute for each host your sessions use,
and when the flyout opens. A host other than github.com is asked only when glab has a login for it.

The card says how the two tools stand:

- **Switched off:** whether each is found (`glab found · gh not found`). Neither is run.
- **Switched on:** for each, the hosts it is logged in to (`✓ glab: gitlab.example.com`), or
  what's missing: `glab not found`, or `gh: not logged in to github.com`. The hosts are the ones
  your sessions' projects are on, and github.com for `gh`.
- **Green** when every tool that's needed is logged in, **yellow** when one is missing or has no
  login. A tool nobody needs (no session's project is on its kind of host) may be missing.

A tool counts as found when Windows can start it from a folder in `PATH`: an `.exe`, `.cmd` or
`.bat`. Details: [Merge requests and pipelines](how-it-works.md#merge-requests-and-pipelines).

### GPG key timer

If git signs your commits with GPG (`commit.gpgsign`), a thin line along the bottom of the widget
and a row in the flyout show whether the signing key is unlocked and about how long it stays so. Off by
default; switched on, it shows nothing while git doesn't sign. Trayward only asks gpg-agent (every 30 seconds):
it never signs and never asks for the passphrase.

The card says how git and GnuPG stand:

- **Switched off:** whether GnuPG (and git) are found. Nothing is run.
- **Switched on:** in green, that git signs with GnuPG and whether the key is unlocked (or that
  gpg-agent isn't running: it starts with the next signature). In yellow, the first thing that's
  missing: git, signing in git (`commit.gpgsign`), signing with GnuPG rather than SSH or X.509
  (`gpg.format`), GnuPG, a key git signs with (`user.signingkey`, or else a secret key), or an
  answer from gpg-agent.

Details: [GPG key timer](how-it-works.md#gpg-key-timer).

## System monitor page

### System monitor

One switch for all of it: your PC's CPU, memory, GPU, network and disk use in the widget, next to
the limits. Off by default. The first time you switch it on, CPU and memory are shown; pick the
values below. Switching it off keeps which values you picked.

It's read once a second while it's on, and not at all while it's off; nothing leaves the PC. The
widget's tooltip shows each value in detail. From 90 % a percentage (or memory in GB that full)
turns amber, from 95 % red. The slim display leaves the monitor out. On the Widgets button
([Position](#position)), values that don't fit are left out a column at a time, and this card
says which. Details:
[System monitor](how-it-works.md#system-monitor).

### CPU

How busy the processor is, in percent. On or off.

### Memory (RAM)

Memory in use: **Off**, **%** or **GB** (counted the way Task Manager does).

### GPU

- **%**: how busy the busiest engine is, as Task Manager shows it.
- **GB**: video memory in use, of the graphics card with the most memory.
- **Off**.

### Network

Bytes sent ↑ and received ↓ per second, over your network adapters. Virtual adapters (such as WSL's
and Hyper-V's) are left out, so nothing counts twice. On or off.

### Disk

- **%**: how much of the time the disks are busy, like "active time" in Task Manager.
- **Read/write**: bytes read and written per second.
- **Off**.

### Graphs

A faint graph of the last 30 seconds behind each value. On by default.

## GitLab and GitHub page

Shown while **Merge and pull requests** is on (Extensions page): how `glab` and `gh` stand, and a
card for each host they're needed for, where you can give Trayward a token of its own.

**Trayward's own token** for a host is used instead of the tool's own login: a narrow, read-only
token just for Trayward, which works even when the tool isn't logged in. Copy it where you create
it, then click **Paste token** on the host's card: Trayward takes it from the clipboard, keeps it
in Windows Credential Manager (as `trayward:gitlab:<host>` or `trayward:github:<host>`, under
"Windows Credentials"), and empties the clipboard. It's handed to `glab` or `gh` only as an
environment variable (`GITLAB_TOKEN`, `GH_TOKEN`), never on their command line, and never written
anywhere else, logged or shown. **Remove** forgets it; the tool's own login is used again. Without
a token of Trayward's, the tool's own login is used, as before.

Each host's card says which login is used there and whether it works: `✓ Trayward's token, until
21/11/2027`, `✓ glab's own login`, or in yellow what's wrong (not logged in, a token that doesn't
work or runs out within 14 days). The hosts are the ones your sessions' projects are on, the ones
Trayward keeps a token for, and github.com; up to three. Their "?" says which token they need,
and leads to the section below.

A token is read-only and runs for a year at most: renew it before it runs out (the host's card
turns yellow 14 days before), and paste the new one.

### Where it shows

Where merge and pull requests and pipelines show: **Both** (the flyout and bubbles, the default), only in the
**Flyout** (the pill under each session and "Your MRs and PRs"), or only as **Bubbles** (when a
pipeline passes, fails or is canceled, or a request is merged; the same switch as
[Pipeline bubbles](#pipeline-bubbles) on the Notifications page). Bubbles need **Bubble** on
(Notifications page).

### GitLab token

For a GitLab host (gitlab.com or your own): a fine-grained personal access token. Make it at
[gitlab.com/-/user_settings/personal_access_tokens](https://gitlab.com/-/user_settings/personal_access_tokens)
(your avatar → **Edit profile** → **Access tokens** → **Add new token**). On a self-hosted GitLab,
put its address in place of `gitlab.com`: `https://gitlab.example.com/-/user_settings/personal_access_tokens`.
Give it an expiry date and these permissions, all read-only:

- For "Groups and projects you're a member of": **Merge request: Read**, **Pipeline: Read** and
  **Project: Read**.
- For "User": **User: Read** (for the login check and your own merge requests).

A 403 `insufficient_granular_scope` names a permission that's missing. A classic token with the
scope `read_api` works too. Copy it, then **Paste token** on the host's card.

### GitHub token

For github.com: a fine-grained personal access token. Make it at
[github.com/settings/personal-access-tokens](https://github.com/settings/personal-access-tokens)
(**Settings** → **Developer settings** → **Personal access tokens** → **Fine-grained tokens** →
**Generate new token**), with an expiry date, "All repositories" (or the ones your sessions work in) and these permissions, all
read-only: **Pull requests: Read**, **Commit statuses: Read** and **Actions: Read** (Metadata comes
by itself). Copy it, then **Paste token** on github.com's card.

Copilot's monthly limit (Providers page) doesn't use this token: it's read with gh's own login
(`gh auth login`): the token above is made for pull requests and has no permission for Copilot.

### glab

GitLab's command-line tool: Trayward runs it for merge requests and pipelines, with its own token
for the host or else glab's own login (`glab auth login`). The card shows its version and where it
is; when it isn't found, the command that installs it: `winget install --id GLab.GLab`.

### gh

GitHub's command-line tool: Trayward runs it for pull requests and checks, with its own token for
github.com or else gh's own login (`gh auth login`), and for Copilot's monthly limit (Providers
page) with gh's own login only. The card shows its version and where it is; when it isn't found, the command
that installs it: `winget install --id GitHub.cli`.

## Away page

### Away mode

While you're away from the PC, a session that needs you (or is done, see [Done too](#done-too)) is
pushed to your phone too, through the [push service](#push-service). Off by default, because it
sends to an address outside your PC. Only the session's name and state go out, e.g. "shop needs
you": no prompts, replies or paths. Snoozing notifications stops the pushes too; presenting and Do
not disturb don't. Details: [Away mode](how-it-works.md#away-mode).

### Away after

How long without mouse or keyboard counts as away: **5**, **10** (the default), **15** or **30**
minutes. A locked PC gets there the same way.

### Done too

On (the default): a session that's done is pushed too. Off: only a session that needs you.

### Push service

Where the pushes go. Click **Edit**, pick the service, and fill in what it needs:

- **ntfy** ([ntfy.sh](https://ntfy.sh) or your own server): the server (empty: `https://ntfy.sh`), the
  topic, and an access token (`tk_…`) only for a protected topic.
- **Gotify** ([gotify.net](https://gotify.net), your own server): the server's address, and the token
  of an app you made for Trayward in Gotify (**Apps** → **Create application**). The token goes in
  the request's `X-Gotify-Key` header, not in the address.
- **Discord**: a channel's webhook address (the channel's settings → **Integrations** →
  **Webhooks**).
- **Slack**: an incoming webhook's address (`https://hooks.slack.com/…`).
- **Webhook**: any address that takes JSON (`{"title", "message", "source"}`).

**Save** keeps it (Enter too), **Cancel** or Escape doesn't. **Paste address** fills the form from a
copied address and picks its service: an ntfy topic's
(`https://ntfy.sh/your-topic`), Gotify's message address with its token
(`https://gotify.example.org/message?token=…`), or a webhook's. `https://`, and `http://` for a server
on your own network (a Gotify or ntfy at home): that one isn't encrypted, so the card says
"Potentially insecure" in yellow; use it only on a trusted local network.

It's kept encrypted for your Windows account, and the card shows only the service and, for ntfy,
Gotify and a JSON address, its host:
topics, tokens and webhook addresses are secrets. For the same reason **Edit** shows the saved server
but not the secrets; left empty, they stay as saved. **Test** sends "Trayward can reach your
phone." and shows whether the server took it (a 401 or 403 means the token is wrong). **Remove**
(red) forgets the service after asking.

## MQTT page

### MQTT

Publishes your limits, plan and sessions (names and states) to your MQTT broker, and an event when
a session needs you or is done, a limit gets close or is free again. Home Assistant finds it by
itself (MQTT discovery): a "Trayward" device with sensors for the limits and sessions. Off by
default. The card says whether it's connected, or why not. Only names and states go out: no prompts,
titles or paths. Topics and entities:
[MQTT and Home Assistant](how-it-works.md#mqtt-and-home-assistant).

### Broker

Click **Edit** and fill in the broker as Home Assistant's MQTT settings show it:

- **Protocol:** `mqtt://`, or `mqtts://` for TLS.
- **Address:** the broker's host name or IP address, e.g. `homeassistant.local` or
  `192.168.1.10`.
- **Port:** empty for the default, 1883 (8883 with TLS).
- **User name** and **Password:** empty when the broker takes anyone. A saved password isn't shown;
  leaving the field empty keeps it.

Enter saves, Escape cancels, Tab goes to the next field. **Paste address** fills the fields from a
copied `mqtt://user:password@host:1883` (or `mqtts://…:8883`). **Remove** (red) forgets the broker
and its password after asking, and Trayward disconnects. The password is kept encrypted for your Windows account. **Test**, shown while MQTT is
on, connects now and the card says how it went. With TLS, Windows checks the broker's certificate:
a self-signed one works once its CA is in Windows' trusted root certificates.

## Mailbox page

### Mailbox

For sessions that can't message each other, like one in Windows and one in WSL, and leave Markdown
messages in each other's inbox folders instead. Trayward shows what's unread for whom and what was
handled: in the flyout, as `✉ 2` on a session's row, and in the tooltip. Off by default; most people
won't need it. Trayward only reads the folder (every 10 seconds), and nothing leaves the PC. The
whole feature: [The session mailbox](mailbox.md).

### Folder

Where the mailbox is. Trayward looks for a folder called `.claude-mailbox` in the folders above
your sessions' working folders, then where it found one before (so it stays when no session runs
there), then in your user folder; a green tick shows the one it found. If
yours is somewhere else or has another name, copy its path (in Explorer: right-click the folder →
**Copy as path**) and click **Paste**. **Find it** goes back to looking for it. Below it, a table
lists the inboxes: unread, handled, when the last one was handled, and whether a session runs for
it.

## Custom extensions page

Programs of your own can show what they do in Trayward: a backup's progress, when a job runs next,
a build's state. Each writes a JSON file; Trayward only reads it and shows rows in the tooltip and
the flyout. The format, with examples: [Your own extensions](extensions.md). The page shows the
folder and the extensions found in it; it's there while **Your custom extensions** is on.

### Your custom extensions

On the Extensions page: switches reading the folder on or off, and with it this page. Off by
default. Trayward never runs, opens or sends anything from these files, and they can't make it do
anything: no links, no commands, no notifications. A file can only say what's shown, within limits
(5 files, 8 rows each for the tooltip and the flyout, 100 characters per text).

### Extensions folder

`%LOCALAPPDATA%\Trayward\extensions`; **Open** shows it in Explorer. Each extension's file found
there gets a card with its label, its id, file name and description, and a switch, kept by its id:
off until you switch it on, so a new file shows nothing until you've looked at it. A file Trayward can't read shows why in
red (not valid JSON, a missing field, a wrong value), and its row says which. Trayward reads the
folder when a file in it changes, and once a minute besides.

## About page

### Check for updates

Once a day, Trayward asks GitHub (`api.github.com`, without a login) for its newest release. A newer
version shows here and in the right-click menu, and a bubble tells you about it once. Nothing is
downloaded or installed: **Download** opens the release page. On by default. **Check now** asks
right away. Details: [Update check](how-it-works.md#update-check).

### Location

The folder `trayward.exe` is in. **Open** shows it there in Explorer, for replacing it with a newer
version (exit Trayward first: right-click → Exit) or for moving it. Moved, it only needs to be
started once from its new place: "Start with Windows" and the hooks then point there by themselves.
