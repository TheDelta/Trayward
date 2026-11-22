# Security policy

## Supported versions

Security fixes go into the latest release. Please update to it before reporting.

| Version          | Supported |
| ---------------- | --------- |
| latest release   | ✅        |
| earlier releases | ❌        |

## Reporting a vulnerability

**Please don't report security problems in public issues or discussions.**

Report them privately through GitHub:
[**Report a vulnerability**](https://github.com/TheDelta/Trayward/security/advisories/new)
(repository → **Security** → **Report a vulnerability**).

Please include:

- what's affected (the widget, the hooks, reading the agents' files, the statistics,
  the release),
- how to reproduce it, ideally with a minimal example,
- the impact you see, and the Trayward version (Settings → About).

Leave out real tokens, your login file and private paths.

Trayward is maintained in spare time, so responses are best effort. You'll get an
acknowledgement as soon as possible, and a fix and an advisory once the problem is
confirmed. Credit is given in the advisory unless you prefer otherwise.

## Scope

Trayward reads your AI coding agents' files, and Claude Code's login to fetch your
usage, so security problems matter even though it is a small app. In scope, for example:

- your login or other data sent anywhere but `api.anthropic.com`, or the login
  written or shown,
- changes to files other than the ones the README lists under "Privacy and
  security",
- the hook mode (`trayward.exe trayward-hook`), which Claude Code runs with data from
  your sessions,
- ways for another program or user to make Trayward run code or open files,
- the published `trayward.exe` not matching this repository's code (each release
  has a build provenance attestation).

Out of scope: vulnerabilities in Claude Code, Codex CLI, GitHub Copilot, VS Code, Windows or other software
Trayward works with (please report those to their vendors).
