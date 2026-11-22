# Roadmap

What's planned for Trayward, what waits for later and what still needs a look. Nothing here is a
promise or has a date. Ideas are welcome: open a
[feature request](https://github.com/TheDelta/Trayward/issues/new?template=feature_request.yaml)
or start a discussion.

What's done is in [CHANGELOG.md](CHANGELOG.md); what Trayward can't do on purpose or for now is
under "Known limitations" in [AGENTS.md](AGENTS.md).

## Next

- **Drop the katex override** in `package.json` (`katex@<0.18.2`) once markdownlint brings
  katex 0.18.2 or newer itself: it comes through `micromark-extension-math`, which still asks for
  katex `^0.16.0`, and 0.16 has no fixed release.

## Later

- **Code signing:** a signed exe, so SmartScreen doesn't warn and Smart App Control doesn't block
  it.
- **More providers:** more AI coding agents, such as Gemini CLI (see "Providers and platforms" in
  [AGENTS.md](AGENTS.md)).
- **Other operating systems:** the logic is in its own crate (`core/`), and what it needs from the
  system goes through `core/src/os/`. Next: the drawing behind an interface the flyout, settings and
  statistics windows can share; then macOS (a menu bar item, Claude Code's login from the Keychain,
  a signed and notarized app); for Linux first a mode without windows that hands the numbers to
  panel tools (Waybar, Polybar, GNOME and KDE extensions) as JSON.
- **More languages** than English and German.
- **Limit resets:** when claude.ai offers limit resets (a weekly or a Fable reset), show how many
  ("2× resets") after Weekly in the flyout, opening the usage page. Waits for the usage endpoint
  to report them: today it doesn't. Only when resets are there, never as a button that's always
  shown.
- **Codex and Copilot sessions:** jump to them like to Claude Code's.
- **Clippy's `indexing_slicing`:** text is no longer cut by position (`string_slice` is on) and no
  list is cut by a range. About 100 places still take a list's item by its number (the week's seven
  days, the logo's path numbers, a click's place among the clickable areas); each was read and
  can't be out of range. The lint that would keep it so also flags every nested JSON lookup by name
  (about 70; a missing name gives null, never a panic), so those would have to be rewritten first.
- **A week's own page:** a week in the statistics' Weeks list that opens the overview of that
  week (the overview knows today, the last 7 or 30 days and all time, no other range).
- **Memory:** idle use is about 15 to 22 MB private; Windows' software renderer keeps about 12 MB
  more after the flyout was open. Worth a look only if it matters to someone.

## To investigate

Parts that are tested in code but haven't been seen working for real yet.

- **Other taskbar sizes:** a taskbar that isn't the usual height (smaller taskbar buttons, tablet
  mode, tools that resize it) or width (at the left or right): whether the widget's rows, dots
  and texts still fit and sit in the middle.
- **Room on the taskbar without the Widgets button:** placed by the tray, the widget can cover app
  buttons when the taskbar is full. Only Position "Widgets" keeps room for it, and that needs
  Windows' Widgets button. Windows has no API that reserves taskbar space for another app, so a
  better way is still to be found.
- **A PC without Claude Code** (no `~/.claude`), with only Codex or Copilot.
- **Codex CLI, Copilot and other agents:** Trayward has only been used with Claude Code so far;
  Codex's and Copilot's limits, sessions and statistics are only tested in code. Among them:
  Copilot's 2-second pause that ends a turn (on an answer with several turns) and its tool names
  other than `powershell` and `ask_user` in the statistics.
- **Other Claude plans:** only tried with Pro and Ultra so far. Max, Team and Enterprise may show
  their limits differently.
- **The usage endpoint's new fields** (`limits`, `weekly_scoped_shares`, `spend`, and the
  code-named ones that are still empty): whether one of them starts carrying the limit resets.

Found by reading the code, not seen happen:

- **Switching to a session in VS Code** sends the session's address once VS Code is in front. With
  several windows of one VS Code, that can be another window than the session's, when the right one
  takes long to come up.
- **Codex thinking without writing** for more than 15 seconds shows as idle until it writes again.
- **A session whose prompt cache lasts 5 minutes** (not the hour Claude Code asks for on a
  subscription): the warning that its cache runs out comes at 10 minutes left, so right after
  "is done". Whether Claude Code still writes such caches, and what the warning should do then.
- **A WSL session whose list file can't be read for a moment** is dropped and found again two
  seconds later; a "done" that fell into that moment isn't announced.
- **Merge requests on a GitLab reached by `http://`** are listed but don't open: links are only
  opened when they are `https://`.
