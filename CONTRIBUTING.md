# Contributing to Trayward

Thanks for taking the time to help! Bug reports, ideas, documentation fixes and code
are all welcome.

## Before you start

- **Questions** ("how do I…?", "why does…?") belong in
  [Discussions → Q&A](https://github.com/TheDelta/trayward/discussions/categories/q-a),
  not in issues.
- **Bugs and feature requests:** please search the
  [existing issues](https://github.com/TheDelta/trayward/issues) first, then use
  one of the issue forms.
- **Security problems:** please don't open a public issue. See
  [SECURITY.md](SECURITY.md).
- **Bigger changes:** open an issue or a discussion first, so we can agree on the
  approach before you spend time on it. Small fixes can go straight to a pull
  request.

Everyone taking part is expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Development setup

You need Windows 11, [Rust](https://rustup.rs/) 1.85 or newer with the MSVC toolchain,
the Windows SDK (part of the Visual Studio Build Tools; `build.rs` compiles the icon,
manifest and version information with its `rc.exe`), Node.js 24 with npm 11 (`devEngines` in
`package.json`; `npm ci` stops on another Node) and Python 3.11 or newer.

```powershell
git clone https://github.com/TheDelta/trayward.git
cd trayward
npm ci                                   # the tools for docs and configs, and the Git hooks
python -m venv .venv                     # the Python tools (Ruff, zizmor, actionlint, ShellCheck)
.venv\Scripts\pip install --require-hashes -r requirements-dev.txt
cargo build --release                    # target\release\trayward.exe
.\target\release\trayward.exe --demo     # made-up sessions and usage, nothing of yours
```

While working on it:

```powershell
npm start        # closes the running Trayward, builds the release and starts it
npm run demo     # builds and starts the demo, next to the real one
npm run stop     # closes the running Trayward (as Exit in its menu: it saves its state)
npm run build    # builds the release only
```

The tools are found in `node_modules` and `.venv` automatically; nothing needs to be
activated. In VS Code, accept the recommended extensions (`.vscode/extensions.json`)
to see the same findings while you type.

[AGENTS.md](AGENTS.md) explains how Trayward works and the rules for changing it.
The detail is in `docs/`: [how it works](docs/how-it-works.md) (what the app reads and
how, with a map of the source files) and
[releases, CI and tooling](docs/maintenance.md). Read the parts that touch your change
before you start.

## Ground rules

- **Read-only where possible.** Trayward never writes Claude Code's login and only
  writes its own hook entries in Claude Code's settings. Please keep it that way.
- **Defensive parsing.** Claude Code's files are internal and change between versions.
  Skip what you don't understand instead of failing, and add a unit test with a small
  sample line for every format you parse.
- **One thread for the UI.** Window procedures turn messages into events and `App`
  handles them one at a time (`app.rs`). Anything slow (network, reading transcripts,
  sizes on disk, UI Automation) runs on a background thread and posts its result back.
- **No personal data in the repository**: no tokens, project names, paths or
  screenshots of your own sessions. Use the demo (`--demo`) for screenshots.
- **Match the surrounding code.** Small helpers, plain names, short comments that
  explain _why_. Unsafe code needs a `// SAFETY:` comment saying why it is sound
  (`unsafe_op_in_unsafe_fn` is denied). Formatting isn't a matter of taste here:
  rustfmt (Rust), Prettier (Markdown, YAML, JSON, JavaScript) and Ruff (Python)
  decide, `npm run format` applies them.
- **YAML files end in `.yaml`**, everywhere (workflows, Dependabot, issue forms, tool
  configs).
- **New dependencies are rare.** Trayward is the `windows` crate and serde; a new
  crate needs a good reason, and its license must be MIT or Apache-2.0.

## Checks

```powershell
npm run check      # everything CI checks, then the tests
npm run format     # fix what can be fixed automatically (formatting, simple lint)
npm test           # only the tests (Rust, then the release scripts)
```

`npm run check` runs rustfmt, Clippy (warnings are errors), Prettier, markdownlint,
CSpell, Ruff (lint and format), actionlint and zizmor (GitHub Actions), a line-ending
check, the Rust tests and the release scripts' tests (`scripts/check.mjs`). CI runs the
same, with Rust on Windows on the oldest supported and the newest Rust. What each
workflow does, and what to do when one fails:
[.github/workflows/README.md](.github/workflows/README.md).

**Git hooks** (installed by `npm ci`, via Husky):

- **pre-commit:** formats and lints only the staged files and stages the fixes
  (`lint-staged.config.mjs`).
- **commit-msg:** the message follows Conventional Commits (below).
- **pre-push:** Clippy, then the Rust and release script tests.

CSpell knows the project's own words from `cspell.config.yaml`; add new ones there
(keep the list sorted).

**Tests** need no network and no Claude Code: they run against small sample lines and
temporary folders. New logic comes with a test where it can be tested without the
Windows UI (parsing, statistics, forecasts, settings). For visible changes, also try
them in the demo, in light and dark mode, and say so in the pull request.

**Smart App Control.** With Smart App Control on, Windows sometimes blocks a fresh
unsigned build or test binary ("An Application Control policy has blocked this file",
os error 4551). Building again with a different optimisation level gives a different
binary, e.g. `$env:CARGO_PROFILE_DEV_OPT_LEVEL = 1; cargo test`.

## Pull requests

1. Fork the repository and create a branch from `main`.
2. Keep the change focused: one fix or feature per pull request.
3. Give it a title that works as a line in the release notes: the release adds it
   to [CHANGELOG.md](CHANGELOG.md) with your name (see below). No need to edit the
   changelog yourself.
4. Update the documentation (README, `docs/`, AGENTS.md) when behaviour, settings or
   what the app reads and writes change.
5. Fill in the pull request template: what changed, why, and how you tested it.
   Screenshots of the demo help for visible changes.

### Commit messages and pull request titles

[Conventional Commits](https://www.conventionalcommits.org): `type(scope): summary`,
lowercase, imperative, no full stop.

| Type                                                | For                                       | In the release notes |
| --------------------------------------------------- | ----------------------------------------- | -------------------- |
| `feat`                                              | a new feature (minor release)             | 🚀 Features          |
| `fix`                                               | a bug fix (patch release)                 | 🐛 Fixes             |
| `perf`                                              | faster, nothing else changes              | ⚡ Performance       |
| `docs`                                              | documentation only                        | 📚 Documentation     |
| `refactor`, `style`, `test`, `build`, `ci`, `chore` | code changes users don't see; CI, tooling | 🧰 Maintenance       |

The scope can move a line: `deps` / `deps-dev` go to 📦 Dependencies, `security` to
🔒 Security, a `feat` or `fix` with `i18n` to 🌐 Translations, `ci` to 🧰 Maintenance
(even a fix: users don't see it), and a breaking change (`!`) to ⚠️ Breaking changes. `fix(flyout): sessions without a title` becomes
"**flyout:** Sessions without a title by @you in #12", plus "fixes #9" when the pull
request closes an issue (write `Fixes #9` in its description).

**Several changes in one pull request:** the squash description can list more,
one per line in the same format. GitHub's squash dialog already fills it with the
branch's commits (`* fix(stats): empty weeks`); keep the ones users should read about
and delete the work-in-progress ones before merging. Each gets its own line in the
release notes, linked to the same pull request. A line with the title's type and scope
counts as the title's own change, and prose lines don't count.

In VS Code, the recommended **Conventional Commits** extension builds the message
step by step (Source Control → its icon), so it always fits.

Scopes are optional; commitlint accepts these (`commitlint.config.mjs`): `widget` (in
the taskbar), `flyout`, `sessions` (finding and following sessions), `usage` (limits,
forecast), `stats` (statistics), `settings`, `notifications` (bubbles, sounds),
`hooks`, `i18n` (translations), `demo`, `ci` (workflows), `security`, `release`,
`deps`, `deps-dev`. A
breaking change gets a `!` (`feat!: settings in a file`) and a `BREAKING CHANGE:` line
in the description.

Pull requests are **squash merged**: the title becomes the one commit on `main`, so
the title is what counts (checked in CI; if it fails, a comment on the pull request
says how to fix it); commits on your branch can be work in progress (the commit-msg
hook still asks for the format, `git commit --no-verify` skips it). The title is
spell-checked too, as it becomes a line in `CHANGELOG.md`: a word CSpell doesn't know
goes into `cspell.config.yaml` (sorted) in the same pull request. Lines added in the
squash description aren't checked until Prepare release: it names unknown words at the
top of the release pull request; fix them on the release branch. One pull request, one
change: if a title needs "and", it's probably two.

## Releases (maintainers)

Nobody tags by hand:

1. **Actions → Prepare release → Run workflow** with `patch`, `minor`, `major`, a
   release candidate (below) or an exact version. It adds a line to `[Unreleased]`
   in `CHANGELOG.md` for every pull request merged since the last release (sorted
   into 🚀 Features, 🐛 Fixes, … by their titles, with authors and fixed issues),
   sets the version in `Cargo.toml` and `Cargo.lock`, and opens the pull request
   "chore(release): v1.2.0" with the notes.
2. Check the notes and squash merge. To change them, edit `CHANGELOG.md` on that
   branch: reword lines, add highlights at the top, and **describe breaking
   changes** under "⚠️ Breaking changes" (what users have to do). The comment under
   `[Unreleased]` lists the sections. Lines can also be written in any pull request:
   one that mentions a pull request (#12) replaces its generated line.
   **Keep the commit title as GitHub suggests it** (`chore(release): v1.2.0 (#12)`):
   only a commit with that title publishes a release. If the notes list breaking
   changes or features but the version is a patch (or breaking changes in a minor one
   from 1.0 on), a warning on top of the pull request says so.
   **Something merged after Prepare release?** Its line would be in no release's
   notes, so the release pull request's "Release notes" check fails. Run Prepare
   release again with the same version (it replaces the branch, so edits made there
   are lost), or add the line to `CHANGELOG.md` on the branch.
3. The merge publishes the release: `trayward.exe` is built on Windows and attached
   to the GitHub release with its SHA-256 sum and a build provenance attestation,
   and the tag is created.

The first release may keep the version `Cargo.toml` already has: run Prepare release
with that exact version (e.g. `0.1.0`).

**Release candidates** (pre-releases, like npm's bumps):

| Input                                | From `1.2.0`                               | From `1.3.0-rc.2`                           |
| ------------------------------------ | ------------------------------------------ | ------------------------------------------- |
| `prepatch` / `preminor` / `premajor` | `1.2.1-rc.1` / `1.3.0-rc.1` / `2.0.0-rc.1` | `1.3.1-rc.1` / `1.4.0-rc.1` / `2.0.0-rc.1`  |
| `rc`                                 | refused: start with a `pre…` input         | `1.3.0-rc.3`                                |
| `patch` / `minor` / `major`          | `1.2.1` / `1.3.0` / `2.0.0`                | `1.3.0` / `1.3.0` / `2.0.0` (the final one) |

`python scripts/release.py next rc` shows what a bump would give. Candidates
collect their lines under `[Unreleased]` (each `rc` adds only what's new); the
final release moves them all into its section.

The GitHub release shows the same notes with @mentions (and the contributors'
avatars) and a "Full Changelog" link: `CHANGELOG.md` is the one place to edit.

## AI-assisted contributions

Trayward itself is developed with AI assistance, and AI-assisted contributions are
welcome too. You're responsible for what you submit: understand it, test it, and keep
it to the change at hand. Please mention in the pull request if a substantial part
was generated.

## License

By contributing, you agree that your contributions are licensed under the project's
[MIT License](LICENSE).
