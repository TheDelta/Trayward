# Maintenance: versions, releases, CI and tooling

The decisions behind the release flow, the workflows, the dependencies and the tool
settings. How to use them: [CONTRIBUTING.md](../CONTRIBUTING.md) and
[.github/workflows/README.md](../.github/workflows/README.md).

## Versions and releases

- **SemVer** (settings, the command-line flags `--demo` and `trayward-hook`, the hook
  entries in Claude Code's settings and where Trayward keeps its data are contracts;
  a major version means one of them breaks). `trayward-terminal`, which Trayward starts itself
  with to look at a terminal, is not one: nothing else calls it.
- **Single source:** `version` in `Cargo.toml`'s `[package]` (with `repository`, the
  URL the changelog links to). The app shows it from `CARGO_PKG_VERSION` (settings
  window), and the exe carries it in its version information (Properties → Details:
  version, author, copyright, description), which `build.rs` writes from `Cargo.toml`'s
  `version`, `authors`, `description` and `homepage` and `LICENSE`'s copyright line. `package.json` has no version (private, it would drift). `Cargo.lock`
  follows it (`release.py` updates both).
- **CHANGELOG.md** (Keep a Changelog's layout, generated, editable): Prepare release
  adds a line per pull request since the last tag (`scripts/changelog.py`, from the
  squash titles on `main`), sorted by type into emoji sections (⚠️ Breaking changes,
  📝 Notes, hand-written ones, 🚀 Features, 🐛 Fixes, ⚡ Performance, 🌐 Translations
  for `feat` / `fix` with `i18n`, 🔒 Security for `security`, 📚 Documentation, 📦 Dependencies for `deps` / `deps-dev`,
  🧰 Maintenance, also for `ci` whatever the type), "**scope:** Text by [@author] in
  [#12] (fixes [#9])" with explicit links (Markdown files don't autolink); author, bot
  flag and closing issues from GraphQL with the workflow's token. Commits without a
  pull request get their short SHA. Release commits are left out, and so is the first
  commit before the first release (the published code as a whole, which
  `[Unreleased]` describes by hand). Lines of a squash description that are
  Conventional Commits themselves (`* fix(stats): …`, known types only) become lines
  of their own with the same pull request (`changes()`), except one with the title's
  type and scope (GitHub's dialog repeats the title as its first line) and repeats. A
  pull request or SHA already mentioned in `[Unreleased]` (outside comments) isn't
  added again, so hand-written lines win. Titles and their scopes go in as literal text
  (`changelog.plain`): tags like `<synthetic>`, @names and bare addresses in code
  spans, `*`, `_`, `<`, `>`, `[`, `]`, `\` and a backtick without its partner escaped,
  so nothing turns into HTML, emphasis, a link or a ping, and markdownlint (MD033,
  MD034) and Prettier pass. An HTML comment under `[Unreleased]`
  (`changelog.TEMPLATE`) says where highlights and breaking changes go; it's kept
  there and left out of a version's section and of the notes. `notes --github`
  (release page) turns the links back into @mentions / #refs (avatars, hovercards;
  bots keep their `apps/` link) and appends "Full Changelog". The same scripts as
  Marqueefin's, with the version read from Cargo instead of Python.
- **Release flow** (no manual tagging):
  1. Actions → **Prepare release** (`prepare-release.yaml`, input `patch` / `minor` /
     `major` / `rc` / `prepatch` / `preminor` / `premajor` / an exact version).
     `scripts/release.py prepare` adds the changelog lines, sets the version in
     `Cargo.toml` and `Cargo.lock` and, for a final version, turns `[Unreleased]`
     into `[x.y.z] - <date>` with a fresh `[Unreleased]` above it and compare links at
     the bottom. It pushes `release/vX.Y.Z` and opens the pull request
     "chore(release): vX.Y.Z" (unknown words in the new notes in a warning box on top:
     they'd fail its CSpell check) with the notes.
  2. Review (edit the notes on that branch if needed) and squash merge.
  3. **`release.yaml`** sees a push to `main` titled `chore(release): vX.Y.Z` (the
     merged release pull request; other pull requests that touch the version publish
     nothing) that changed the version to that untagged one (the first release may
     keep it): builds `trayward.exe` on Windows (`--locked`, no cache), adds its
     SHA-256 sum, the licenses of the crates in it and a build provenance attestation, then `gh release create --target
<sha>` creates the tag and the release from the changelog section with the exe.
     Notes are checked before anything is built or published (in "Is this a
     release?"). Prepare release refuses when nothing was merged since the last
     release (`main`'s newest commit carries a release tag), except to make that
     candidate final. It also warns on top of the pull request when the bump looks too
     small for the notes (`release.py check-bump`: breaking changes need a major
     version, a minor one before 1.0; features at least a minor one).
  4. **A pull request merged between Prepare release and the release merge** would
     reach no changelog: the tag goes on the release merge, and the next release only
     looks at what came after it. So `pull-request.yaml`'s "Release notes" job (on
     `release/v*` branches only, skipped elsewhere) runs `release.py missing`: the
     pull requests merged after the release commit that the notes don't mention.
     Re-running Prepare release adds them (and replaces the branch); a hand-written
     line works too. Lines removed from the notes by hand before that stay removed.
  5. **Queues, not cancellations:** `release.yaml` has no workflow-wide `concurrency`
     (GitHub keeps one pending run per group, so a later push could cancel a waiting
     release); its build and publish jobs each queue in their own group, and only
     real releases reach them. The release exe is built with a pinned Rust (`RUST` in
     `release.yaml`), the one toolchain input of the published exe; raise it by pull
     request.
- **Why it's built this way:** pushes, tags and pull requests made with the
  workflow's `GITHUB_TOKEN` don't start other workflows. So the tag is created in the
  same run that publishes, and Prepare release uses the **release bot**, a GitHub App
  (`actions/create-github-app-token`, environment `release`: `APP_RELEASE_BOT_ID`,
  `APP_RELEASE_BOT_KEY`; the same App as Marqueefin's can be installed here): its
  token is limited to this repository, contents and pull requests; its branch and pull
  request start CI and the title check like anyone's, and it's the author of the
  release commit (never whoever started the run: see the co-author note in the
  workflow). The workflow's own token only reads, the checkout keeps no credentials,
  and the bot's token is made in the last step, for the push and the pull request
  alone: the dev tools from npm that run before it (CSpell) are never near it. The release exe is built without a
  cache (zizmor: cache poisoning), on Windows, and published from Linux, where the
  attestation and `gh` live. It has the C runtime linked in (`.cargo/config.toml`,
  `+crt-static`, for every build): `VCRUNTIME140.dll` isn't part of Windows, and "one
  exe, no runtime" means it starts on a PC without the Visual C++ redistributable. A
  fork doesn't publish (`github.event.repository.fork`): synced with a release commit,
  it would release under its own name.
- **The first release:** the private history isn't published; the public repository
  starts with one commit of the code as it is. Run Prepare release with the version
  `Cargo.toml` has (or a higher one): `release.py` allows the current version while no
  `v*` tag exists, and `release.yaml` releases it although the version didn't change.
- Pushing a tag by hand (`git tag v1.2.3 && git push origin v1.2.3`) still releases;
  the tag must match `Cargo.toml` at that commit.
- **Pre-releases** (`1.0.0-rc.1`): `prepare` adds the new lines to `[Unreleased]`
  and changes the version, but makes no section of its own; the notes come from
  `[Unreleased]` and they become GitHub pre-releases. Bumps work like npm's
  (`minor` of `1.1.0-rc.1` is `1.1.0`; `preminor` of `1.0.0` is `1.1.0-rc.1`, counting
  from rc.1, not npm's rc.0); `rc` is the next candidate (`1.1.0-rc.1` →
  `1.1.0-rc.2`) and refuses a final version.
- `scripts/release.py` (stdlib): `current`, `next <how>`, `prepare <how>`,
  `notes <version>`, `missing`, `check-bump`, `--root <dir>` to work on a copy
  (`scripts/tests/`).
- **Code signing:** not yet. The release has a SHA-256 sum and a build provenance
  attestation (`gh attestation verify trayward.exe --owner <owner>`), which prove
  where the exe was built, but Windows SmartScreen and Smart App Control look for an
  Authenticode signature. A free signing service for open source (e.g. SignPath
  Foundation) would sign it in `release.yaml`, between the build and the release.

## CI and updates

- **One command, everywhere:** `npm run check` (`scripts/check.mjs`) runs every
  check: rustfmt, Clippy, Prettier, markdownlint, CSpell, Ruff lint + format,
  actionlint, zizmor, LF line endings, then the Rust tests and the release scripts'
  tests. CI's "Lint and checks" job runs it without Rust (`--no-rust`), and the Rust
  jobs run the Rust part on Windows; `npm run format` fixes what can be fixed. Tools
  come from `node_modules` and `.venv` (or PATH), no activation needed.
- **Workflows** (overview for humans: `.github/workflows/README.md`; keep it in step):
  - `ci.yaml` (push to main, pull requests, by hand): "Lint and checks" (Linux),
    "Rust (oldest)" / "Rust (newest)" on Windows (`rust-version` from `Cargo.toml` and
    stable; Clippy, tests; the newest also rustfmt, coverage with cargo-llvm-cov and
    the release build as an artifact). The coverage report's file paths are made
    relative to the repository before upload: llvm-cov writes absolute Windows paths,
    which the scanner on Linux couldn't match to the sources. Rust runs on Windows because the app only
    builds there (`build.rs` needs the Windows SDK's `rc.exe`). "Core on Linux" runs Clippy on the core
    crate (`core/`, tests included) on Linux, so the logic keeps building on other systems; it isn't a
    required check.
  - `pull-request.yaml`: "PR title" (commitlint and CSpell: the title becomes a
    CHANGELOG.md line, which CSpell checks; on failure the run summary explains it),
    and "Release notes" on release pull requests (see the release flow above).
    The commit-msg hook spell-checks the first line too.
  - `pr-title-help.yaml` (`pull_request_target`, so forks too): a comment on the pull
    request when the title fails (`scripts/pr_title.py`: commitlint's rules in plain
    words, types and their release-notes sections, scopes, unknown words; words in the
    pull request's own `cspell.config.yaml` and scopes in its `commitlint.config.mjs`
    count, both read as text through the API, never run), updated to "✅ fine now"
    with the release-notes line once fixed; found again by its
    `<!-- trayward:pr-title -->` marker, in a comment of `github-actions[bot]` only
    (anyone could write the marker). The title is quoted as code, so its Markdown,
    @mentions and #refs stay text. Never checks out or runs the pull request's
    code (main's checkout, the title via env); zizmor's dangerous-triggers finding is
    ignored with that reason.
  - `prepare-release.yaml`, `release.yaml`: see above.
  - Weekly, public repository only: `scorecard.yaml` (OpenSSF Scorecard, badge),
    `links.yaml` (lychee, `lychee.toml`). They test `github.repository_visibility`,
    which every event has (a `schedule` payload may lack `repository`).
- **Rulesets** (set up in GitHub, not in the repository) on `main`:
  - `main`: pull requests only, squash, signed and linear history, no force push or
    deletion, and the five job names of ci.yaml and pull-request.yaml ("Lint and
    checks", "Rust (oldest)", "Rust (newest)", "PR title", "Release notes") as
    required checks (a job skipped by its `if` counts as passed), from
    GitHub Actions only, with "require branches to be up to date", so every merged
    state has passed CI (what gets released is what was tested). **No bypass**, not
    even for admins. Renaming a job means updating it.
  - `reviews`: one approval by a code owner (`.github/CODEOWNERS`), on the latest
    push. Admins may bypass it in a pull request: GitHub doesn't let you approve your
    own, and since `main` can't be bypassed, the checks still apply. Pull requests
    from the release bot, Dependabot and contributors need the maintainer's approval
    (a leaked bot key can't merge anything).
  - Code scanning (CodeQL, public repository only) and release tags, in two rulesets:
    `v*` can't move or be deleted, for anyone. Who may create one isn't restricted:
    GitHub accepts its Actions integration as a ruleset's bypass only in an
    organisation's repository, and without it the Release workflow couldn't tag. A tag
    is what gets built and published, so the keys that could make one are kept few: the
    release bot only opens the release pull request, and its key lives only in the
    `release` environment, limited to `main`; releases are immutable once published.
    Pushing a tag by hand stays the maintainer's way in. CodeQL runs as
    GitHub's default setup (no workflow file): Rust (generally available since October
    2025, analysed without a build), Python, JavaScript and the workflows.
- **Repository settings** (Settings → General): squash merging only, its default
  message **"Pull request title and commit details"** (`squash_merge_commit_title:
PR_TITLE`, `squash_merge_commit_message: COMMIT_MESSAGES`): the title is what the
  "PR title" check linted (GitHub's default takes a lone commit's message instead), and
  the commit list fills the description with `* fix: …` lines to keep or delete.
  Auto-merge on, branches deleted after merging, "Update branch" suggested, no wiki or
  projects. Actions: only selected actions (GitHub's, verified creators, and the
  patterns `SonarSource/*`, `lycheeverse/*`, `ossf/*`, `taiki-e/install-action@*`,
  `dependabot/*`), full SHAs required, the workflow token read-only by default. Releases are
  immutable (Settings → General → Releases): once published, the exe and the tag stay as
  they are; `gh release create` uploads the assets before it publishes, so the workflow
  works with it. The picture link previews show is `.github/social-preview.png` (1280×640,
  from the demo's pictures), uploaded under Settings → General → Social preview: GitHub
  has no API for it, so a new one has to be uploaded by hand. Discussions are on, with
  a pinned welcome post in Announcements; the issue forms send questions to Q&A.
- **Supply chain in CI:** `npm ci --ignore-scripts` everywhere (no install script of
  any package runs; a pull request's package.json may come from a fork); `cargo …
--locked` (Cargo.lock decides); Python tools from `requirements-dev.txt`, pinned
  **with hashes** (`pip-compile --generate-hashes` from `requirements-dev.in`;
  Dependabot updates both); the read-only token only in the separate "zizmor (online
  audits)" step. actionlint-py is a source package that downloads the actionlint
  binary when installed (the one tool outside the hashes). cargo-llvm-cov comes
  prebuilt in a fixed version from `taiki-e/install-action` (pinned by SHA), which
  checks the download against its own checksums: building it took minutes of every
  Windows run.
- **Actions are pinned to full commit SHAs** with the version as a comment
  (`actions/checkout@<sha> # v<version>`); Dependabot updates both. `persist-credentials:
false` on every checkout that doesn't push; top-level `permissions` minimal,
  widened per job. zizmor checks all of this (online audits in CI with the read-only
  token, `--offline` locally; `.github/zizmor.yaml` sets the Dependabot cooldown it
  expects to 3 days). Node's version has one source: `devEngines.runtime` in
  `package.json` (`^24`: the newest 24.x LTS). npm refuses `npm ci` with another major
  (`onFail: error`), and `actions/setup-node` (`node-version-file: package.json`)
  installs it in CI.
- **Dependabot** (`dependabot.yaml`, auto-merge for safe updates in
  `dependabot-auto-merge.yaml`): cargo, npm, pip, github-actions; weekly, a **3-day
  cooldown** for new releases (`.npmrc` `min-release-age=3` matches it, so npm
  accepts what Dependabot proposes), Conventional Commits prefixes (`fix(deps)` for
  crates, which ship in the exe, `chore(deps-dev)`, `ci(deps)`), grouped (minor /
  patch). Crate updates stay manual. Auto-merge is turned on with the workflow's
  token, so the merge itself starts no workflows on `main` (no CI run, SonarQube
  analysis or `trayward-<sha>` artifact for that commit): the pull request was green
  and up to date, and the next merge or the weekly runs catch up. The release bot's
  key isn't available to Dependabot's runs, which is why it isn't used there.
- **Everything is pinned exactly:** `Cargo.lock`, npm packages in `package.json` (no
  ranges; `.npmrc` `save-exact=true` for new ones, Dependabot
  `versioning-strategy: increase` moves the pin), Python tools by hash, actions by
  SHA. The one range is `devEngines.runtime` (`^24`).
- **Release supply chain:** a signed GitHub build attestation for `trayward.exe`
  (public repository) and its SHA-256 sum next to it.
- **Rust lints** (`Cargo.toml` `[workspace.lints]`, both crates): Clippy's default set
  as warnings (errors in CI), plus `undocumented_unsafe_blocks`, `unwrap_used`,
  `expect_used`, `panic` (tests may: `clippy.toml`), `dbg_macro`, `todo`,
  `unimplemented`, `print_stdout`, `print_stderr` and `allow_attributes_without_reason`;
  `unsafe_op_in_unsafe_fn` and `unused_must_use` denied; rustdoc's broken and private
  links and stray HTML denied (`cargo doc` in `npm run check` and CI). Not yet:
  `indexing_slicing` and `string_slice` (about 250 places; ROADMAP.md). rustfmt
  (`rustfmt.toml`): 120 columns, `use_small_heuristics = "Max"`.
- **Dependencies** (`deny.toml`, cargo-deny 0.21.0): RustSec advisories (vulnerable,
  unmaintained, yanked), licenses MIT or Apache-2.0 (exceptions by crate name: `foldhash`
  Zlib, `unicode-ident` Unicode-3.0), crates.io only, no wildcard versions; two versions
  of a crate only warn. CI's "Rust dependencies" job installs it prebuilt
  (taiki-e/install-action, checksums); locally `cargo install --locked`. Its version
  follows the 3-day rule like everything else.
- **Third-party licenses** (`scripts/licenses.py`): MIT and Apache-2.0 want their text
  to go with every copy, and the release is one exe. So each release has
  `THIRD-PARTY-LICENSES.txt` next to it: the crates `Cargo.lock` pins for the Windows exe
  (those it's built from, not the ones only tests use) with the license files from their
  own sources, each different text once, and a line for Rust's standard library. It's
  made in the release's build job, not kept in the repository (it would change with
  every crate update). The README links to the latest one, Settings → About →
  **Licenses** to the one of the running version (`update::licenses_url`). The same
  script without `--out` is a check (`npm run check`, CI's "Rust dependencies"): a crate
  that ships no license file fails it, and gets its text in the script's `EXTRA`.
- **Ruff** (`ruff.toml`): F, E9, B, UP, SIM, DTZ, C90 (max complexity 10), S, I, A, C4,
  PIE, RET, PERF, RUF for the release scripts; `S310` (urlopen on GitHub's fixed API
  URL) ignored. **100 columns** for the scripts and docs (Ruff `line-length`, Prettier
  `printWidth`, `.editorconfig`); Markdown keeps its own breaks (release notes join
  them again, see `changelog.unwrap`).
- **CSpell** (`cspell.config.yaml`): British and American English; Win32 names (all
  caps, with underscores) and Segoe Fluent Icons code points are skipped by pattern,
  the project's own words are listed (sorted).
- **Git hooks** (Husky, installed by `npm ci`): pre-commit runs lint-staged (only
  staged files, fixes restaged, one tool at a time; `cargo fmt` for Rust), commit-msg
  runs commitlint and spell-checks the first line, pre-push runs Clippy and the
  tests. CI sets `HUSKY=0`.
- **SonarQube Cloud**, analyzed in CI (the "SonarQube Cloud" job of `ci.yaml`,
  `sonar-project.properties`) for the coverage: automatic analysis can't read coverage
  reports. The Rust (newest) job uploads `coverage-rust` (`cargo llvm-cov --lcov`, paths
  made relative to the repository); the SonarQube job takes it from the same run and
  makes its paths absolute, and passes the report's own path as an absolute one: the scanner
  makes a module of each Cargo package (the app, the core) and looks for the report from each
  module's folder, so the core got none. "Lint and checks" runs the release scripts' tests
  again under coverage.py and uploads `coverage-python` (Cobertura XML) for it too.
  **Coverage** aims at 80 % overall, Sonar's bar for new code: the windows are laid out
  and drawn in tests into memory (`gfx::testing::offscreen`, no window). Left out of it,
  with the reason in `sonar-project.properties`: `app.rs` (event handling and window
  procedures), the Win32 glue that only runs with real windows (`win.rs`, `edit.rs`,
  `menu.rs`, `uia.rs`, `main.rs`) and HTTPS/TLS over WinHTTP and SChannel (network). It runs for `main` and for pull
  requests from this repository only: a fork's pull request gets no secrets in its run,
  and is analyzed once merged. (It used to run on `workflow_run` to analyze forks too,
  checking out their code next to the token; Scorecard and CodeQL rightly flag that
  pattern, whatever the scanner does with the code.) Clippy isn't run by the scanner (it's
  on Linux; CI runs Clippy on Windows).
  Organization and project key are repository variables (`SONAR_ORGANIZATION`,
  `SONAR_PROJECT_KEY`). `main`'s analyses pass `sonar.projectVersion` (read from
  `Cargo.toml` with `sed`, no repository code runs), so with New Code = "Previous
  version" SonarQube shows what each release adds and marks releases in its Activity
  graph. The whole repository is analyzed (SonarQube Cloud ignores `sonar.sources` now), but for
  the release scripts' tests (`sonar.exclusions`). Findings that stay are listed in `sonar-project.properties`
  with their reasons (`sonar.issue.ignore.multicriteria`), not clicked away on the website:
  the release scripts taking file names from their command line, the dev scripts
  starting tools from PATH, and the complexity limit (30) in the six files that draw the
  windows and handle their events (`app.rs`, `flyout.rs`, `settings_window.rs`,
  `stats_window.rs`, `widget.rs`, `tip.rs`), where one function per window is the design. Everywhere
  else the limit holds: a function over it is split. Clippy's `redundant_closure_for_method_calls` is on because
  Sonar asks for the same.
