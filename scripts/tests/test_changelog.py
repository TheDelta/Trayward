"""scripts/changelog.py: CHANGELOG.md lines from the commits since the last
release, and how prepare puts them into [Unreleased]."""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest

# scripts/tests/ -> the repository
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, "scripts"))
import changelog  # from scripts/, added to the path above
import release

GIT = shutil.which("git") or ""  # "": skipped
URL = "https://github.com/o/r"


def commit(subject, body=""):
    return {"sha": "abcdef1234567890", "subject": subject, "body": body}


def fake_lookup(numbers):
    """Pull request details as GitHub's API would give them."""
    people = {12: ("someone", False, [9]), 19: ("dependabot", True, [])}
    return {n: {"author": people[n][0], "bot": people[n][1], "issues": people[n][2]}
            for n in numbers if n in people}  # fmt: skip


def no_details(numbers):
    return {}


class Lines(unittest.TestCase):
    def entries(self, *subjects, body=""):
        return changelog.lines([commit(s) for s in subjects], body, URL, fake_lookup)

    def test_a_feature_with_author_and_issue(self):
        self.assertEqual(self.entries("feat(page): theme switch (#12)"), [(
            changelog.FEATURES,
            "- **page:** Theme switch by [@someone](https://github.com/someone) in "
            "[#12](https://github.com/o/r/pull/12) (fixes [#9](https://github.com/o/r/issues/9))",
        )])  # fmt: skip

    def test_a_title_with_a_list_marker(self):
        # A squash merge that took "* feat(page): …" from the description as its title
        section, text = self.entries("* feat(page): filters tweaked")[0]
        self.assertEqual(section, changelog.FEATURES)
        self.assertTrue(text.startswith("- **page:** Filters tweaked"), text)

    def test_sections(self):
        cases = {
            "fix(seerr): no date (#30)": changelog.FIXES,
            "perf: faster (#31)": changelog.PERFORMANCE,
            "feat(i18n): French (#32)": changelog.TRANSLATIONS,
            "docs: setup (#33)": changelog.DOCS,
            "fix(deps): bump x (#34)": changelog.DEPENDENCIES,
            "chore(deps-dev): bump y (#35)": changelog.DEPENDENCIES,
            "ci: faster checks (#36)": changelog.MAINTENANCE,
            "fix(ci): the checkout (#39)": changelog.MAINTENANCE,
            "fix(security): escape titles (#40)": changelog.SECURITY,
            "refactor: tidy (#37)": changelog.MAINTENANCE,
            "feat!: new settings (#38)": changelog.BREAKING,
            "Update README.md": changelog.MAINTENANCE,
        }
        for subject, want in cases.items():
            with self.subTest(subject=subject):
                self.assertEqual(self.entries(subject)[0][0], want)

    def test_breaking_change_in_the_body(self):
        found = changelog.lines(
            [commit("feat: x (#40)", "BREAKING CHANGE: EXPORT_X is gone")],
            "",
            URL,
            no_details,
        )
        self.assertEqual(found[0][0], changelog.BREAKING)

    def test_release_commits_are_left_out(self):
        self.assertEqual(self.entries("chore(release): v1.2.0 (#41)"), [])

    def test_already_mentioned(self):
        body = "- Theme switch, written by hand ([#12](https://github.com/o/r/pull/12))"
        self.assertEqual(self.entries("feat(page): theme switch (#12)", body=body), [])
        self.assertEqual(self.entries("fix: x", body="see abcdef1"), [])

    def test_bots_packages_and_commits_without_pull_request(self):
        dep = self.entries("fix(deps): bump @floating-ui/dom from 1.7.4 to 1.7.5 (#19)")
        self.assertEqual(dep[0][1], (
            "- Bump `@floating-ui/dom` from 1.7.4 to 1.7.5 by "
            "[@dependabot](https://github.com/apps/dependabot) in [#19](https://github.com/o/r/pull/19)"
        ))  # fmt: skip
        self.assertEqual(self.entries("fix: tweak")[0][1], (
            "- Tweak ([`abcdef1`](https://github.com/o/r/commit/abcdef1234567890))"
        ))  # fmt: skip

    def test_titles_stay_literal_markdown(self):
        for text, want in (
            ("handle <synthetic> model", "Handle `<synthetic>` model"),
            ("a_b*c*d in __init__", "A\\_b\\*c\\*d in \\_\\_init\\_\\_"),
            ("ask @someone, not @org/team", "Ask `@someone`, not `@org/team`"),
            ("keep `<code>` and `a_b`", "Keep `<code>` and `a_b`"),
            ("[link](https://x.example/a) or me@x.example", "\\[link\\](`https://x.example/a`) or `me@x.example`"),
        ):  # fmt: skip
            with self.subTest(text=text):
                line = changelog.line(
                    {**changelog.parse(commit(f"fix: {text}")), "pr": None}, {}, URL
                )
                self.assertEqual(line.split(" ([`")[0], f"- {want}")

    def test_without_details(self):
        found = changelog.lines([commit("feat: x (#50)")], "", URL, no_details)
        self.assertEqual(found[0][1], "- X in [#50](https://github.com/o/r/pull/50)")


class SeveralChanges(unittest.TestCase):
    """A squash description listing more changes, as GitHub's dialog fills it."""

    # The description of #21: the title's own change first (worded a bit
    # differently), prose bullets, trailers
    BODY = """* ci(release): generate the changelog from pull requests; refuse empty releases

- Prepare release adds a CHANGELOG.md line for every pull request.
- Note: prose with a colon.

* feat: better husky messages
* fix(seerr): requests without a date
* fix(seerr): Requests without a date

Co-authored-by: Someone <someone@example.com>"""

    def test_one_line_per_change(self):
        found = changelog.lines(
            [commit("ci(release): generate changelog from pull requests (#21)", self.BODY)],
            "", URL, no_details,
        )  # fmt: skip
        self.assertEqual(found, [
            (changelog.MAINTENANCE, "- **release:** Generate changelog from pull requests in [#21](https://github.com/o/r/pull/21)"),
            (changelog.FEATURES, "- Better husky messages in [#21](https://github.com/o/r/pull/21)"),
            (changelog.FIXES, "- **seerr:** Requests without a date in [#21](https://github.com/o/r/pull/21)"),
        ])  # fmt: skip

    def test_a_hand_written_line_replaces_them_all(self):
        found = changelog.lines(
            [commit("feat: x (#21)", self.BODY)], "- All of it (#21)", URL, no_details
        )
        self.assertEqual(found, [])

    def test_release_commits_stay_out(self):
        found = changelog.lines(
            [commit("chore(release): v1.2.0 (#22)", "* feat: x")], "", URL, no_details
        )
        self.assertEqual(found, [])


class Merge(unittest.TestCase):
    def test_into_existing_and_new_sections_in_order(self):
        body = "Intro.\n\n### 🐛 Fixes\n\n- A fix, by hand.\n\n### Custom\n\nText."
        merged = changelog.add(body, [
            (changelog.MAINTENANCE, "- Chore"),
            (changelog.FIXES, "- Another fix"),
            (changelog.BREAKING, "- Breaking"),
        ])  # fmt: skip
        self.assertEqual(merged, (
            "Intro.\n\n### ⚠️ Breaking changes\n\n- Breaking\n\n### Custom\n\nText.\n\n"
            "### 🐛 Fixes\n\n- A fix, by hand.\n- Another fix\n\n### 🧰 Maintenance\n\n- Chore"
        ))  # fmt: skip

    def test_unwrap(self):
        wrapped = (
            "Intro that was\nwrapped by hand.\n\n### 🚀 Features\n\n"
            "- Export of the whole library to one\n  self-contained page.\n"
            "- Second item\n  1. nested list\n\n"
            "> A quote\n> on two lines\n\n"
            "```sh\nkeep\nthese lines\n```\n\nHard break  \nstays"
        )
        self.assertEqual(changelog.unwrap(wrapped), (
            "Intro that was wrapped by hand.\n\n### 🚀 Features\n\n"
            "- Export of the whole library to one self-contained page.\n"
            "- Second item\n  1. nested list\n\n"
            "> A quote\n> on two lines\n\n"
            "```sh\nkeep\nthese lines\n```\n\nHard break  \nstays"
        ))  # fmt: skip

    def test_for_github(self):
        body = (
            "- X by [@someone](https://github.com/someone) in "
            "[#12](https://github.com/o/r/pull/12) (fixes [#9](https://github.com/o/r/issues/9))\n"
            "- Y by [@dependabot](https://github.com/apps/dependabot)"
        )
        self.assertEqual(changelog.for_github(body), (
            "- X by @someone in #12 (fixes #9)\n"
            "- Y by [@dependabot](https://github.com/apps/dependabot)"
        ))  # fmt: skip


@unittest.skipUnless(GIT, "needs git")
class FromGit(unittest.TestCase):
    """prepare in a real repository: the commits since the last tag."""

    CHANGELOG = (
        "# Changelog\n\n## [Unreleased]\n\n" + changelog.TEMPLATE + "\n\n"
        "### 🐛 Fixes\n\n- By hand.\n\n## [1.0.0] - 2026-10-01\n\n- Everything.\n\n"
        "[1.0.0]: https://github.com/o/r/releases/tag/v1.0.0\n"
    )

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = tmp.name
        self.git("init", "-q", "-b", "main")
        self.write("1.0.0")
        self.commit("chore(release): v1.0.0 (#1)")
        self.git("tag", "v1.0.0")
        self.commit("feat(page): theme switch (#12)")
        self.commit("fix(deps): bump @floating-ui/dom from 1.7.4 to 1.7.5 (#19)")

    def git(self, *args):
        env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com",
               "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.com"}  # fmt: skip
        subprocess.run(
            [GIT, "-C", self.root, "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args],
            check=True, capture_output=True, env=env,
        )  # fmt: skip

    def write(self, version):
        files = {release.VERSION_FILE: f'[package]\nname = "app"\nversion = "{version}"\nrepository = "{URL}"\n',
                 "CHANGELOG.md": self.CHANGELOG}  # fmt: skip
        for name, text in files.items():
            release.write(self.root, name, text)

    def commit(self, subject):
        self.git("commit", "-q", "--allow-empty", "-am" if subject else "-m", subject)

    def test_a_final_release(self):
        release.prepare(self.root, "minor", repo="o/r", lookup=fake_lookup)
        with open(os.path.join(self.root, "CHANGELOG.md"), encoding="utf-8") as f:
            text = f.read()
        unreleased, rest = text.split("## [1.1.0]")
        self.assertIn(changelog.TEMPLATE, unreleased)  # the hints stay on top
        self.assertNotIn("<!--", rest)
        notes = release.notes(self.root, "1.1.0", github=True)
        self.assertEqual(notes, (
            "### 🚀 Features\n\n- **page:** Theme switch by @someone in #12 (fixes #9)\n\n"
            "### 🐛 Fixes\n\n- By hand.\n\n"
            "### 📦 Dependencies\n\n- Bump `@floating-ui/dom` from 1.7.4 to 1.7.5 by "
            "[@dependabot](https://github.com/apps/dependabot) in #19\n\n"
            "**Full Changelog**: https://github.com/o/r/compare/v1.0.0...v1.1.0"
        ))  # fmt: skip

    def test_candidates_add_only_what_is_new(self):
        release.prepare(self.root, "preminor", repo="o/r", lookup=fake_lookup)
        first = release.notes(self.root, "1.1.0-rc.1")
        self.commit("chore(release): v1.1.0-rc.1 (#20)")
        self.git("tag", "v1.1.0-rc.1")
        self.commit("fix: no date (#21)")
        release.prepare(self.root, "rc", repo="o/r", lookup=fake_lookup)
        second = release.notes(self.root, "1.1.0-rc.2")
        self.assertTrue(second.startswith(first.split("### 🐛 Fixes")[0]))
        self.assertEqual(second.count("#12"), 1)
        self.assertIn("- No date in [#21](https://github.com/o/r/pull/21)", second)

    def test_a_pull_request_merged_after_prepare_release(self):
        """What reaches main between Prepare release and the release merge is missing
        from the notes; lines removed by hand before that stay removed."""
        release.prepare(self.root, "minor", repo="o/r", lookup=fake_lookup)
        self.git("switch", "-q", "-c", "release/v1.1.0")
        self.commit("chore(release): v1.1.0")
        self.assertEqual(release.missing(self.root, "1.1.0"), [])
        self.git("switch", "-q", "main")
        self.commit("fix: late (#22)")
        self.git("switch", "-q", "release/v1.1.0")
        self.git("merge", "-q", "--no-edit", "main")  # "Update branch"
        lost = release.missing(self.root, "1.1.0")
        self.assertEqual(
            lost, [(changelog.FIXES, "- Late in [#22](https://github.com/o/r/pull/22)")]
        )
        self.assertEqual(release.missing(self.root, "9.9.9"), [])  # not a release branch

    def test_the_file_stays_lint_clean(self):
        """Before the first final release [Unreleased] ends the file: one newline, no
        blank line (MD012, MD047)."""
        self.CHANGELOG = self.CHANGELOG.split("## [1.0.0]")[0].rstrip() + "\n"
        self.write("1.0.0")
        release.prepare(self.root, "preminor", repo="o/r", lookup=fake_lookup)
        with open(os.path.join(self.root, "CHANGELOG.md"), encoding="utf-8") as f:
            text = f.read()
        self.assertTrue(text.endswith(")\n"), repr(text[-40:]))
        self.assertNotIn("\n\n\n", text)


if __name__ == "__main__":
    unittest.main()
