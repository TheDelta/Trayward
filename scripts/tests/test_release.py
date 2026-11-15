"""scripts/release.py: version bumps, the CHANGELOG rewrite and release notes."""

import contextlib
import io
import os
import re
import sys
import tempfile
import unittest

# scripts/tests/ -> the repository
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, "scripts"))
import release  # from scripts/, added to the path above

CHANGELOG = """# Changelog

Intro.

## [Unreleased]

### Fixed

- A fix.

## [1.0.0] - 2026-10-01

### Added

- Everything.

[Unreleased]: https://github.com/o/r/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/o/r/releases/tag/v1.0.0
"""
CARGO = """[package]
name = "app"
version = "{}"  # comment
repository = "https://github.com/o/r"

[dependencies.windows]
version = "0.62.2"
"""
LOCK = """[[package]]
name = "aaa"
version = "1.0.0"

[[package]]
name = "app"
version = "{}"
"""


class Bump(unittest.TestCase):
    def test_final_versions(self):
        for how, want in (("patch", "1.2.4"), ("minor", "1.3.0"), ("major", "2.0.0")):
            self.assertEqual(release.bump("1.2.3", how), want)

    def test_from_a_pre_release_like_npm(self):
        self.assertEqual(release.bump("1.0.0-rc.1", "patch"), "1.0.0")
        self.assertEqual(release.bump("1.0.0-rc.1", "minor"), "1.0.0")
        self.assertEqual(release.bump("1.0.0-rc.1", "major"), "1.0.0")
        self.assertEqual(release.bump("1.2.3-rc.1", "minor"), "1.3.0")

    def test_release_candidates(self):
        self.assertEqual(release.bump("1.0.0-rc.3", "rc"), "1.0.0-rc.4")
        self.assertEqual(release.bump("1.0.0-rc.9", "rc"), "1.0.0-rc.10")
        for current, how, want in (
            ("1.2.0", "prepatch", "1.2.1-rc.1"),
            ("1.2.0", "preminor", "1.3.0-rc.1"),
            ("1.2.0", "premajor", "2.0.0-rc.1"),
            ("1.0.0-rc.3", "prepatch", "1.0.1-rc.1"),
            ("1.0.0-rc.3", "preminor", "1.1.0-rc.1"),
            ("1.0.0-rc.3", "premajor", "2.0.0-rc.1"),
        ):
            with self.subTest(current=current, how=how):
                self.assertEqual(release.bump(current, how), want)

    def test_rc_needs_a_release_candidate(self):
        for current in ("1.2.0", "1.0.0-beta.1"):
            with self.subTest(current=current), self.assertRaises(SystemExit) as e:
                release.bump(current, "rc")
            self.assertIn("prepatch, preminor or premajor", str(e.exception))

    def test_exact_versions_must_be_newer(self):
        self.assertEqual(release.bump("1.0.0-rc.1", "1.0.0-rc.2"), "1.0.0-rc.2")
        for older in ("1.0.0-rc.1", "0.9.0", "1.0.0-beta.1"):
            with self.subTest(older=older), self.assertRaises(SystemExit):
                release.bump("1.0.0-rc.1", older)
        with self.assertRaises(SystemExit):
            release.bump("1.0.0", "v1.1")

    def test_precedence(self):
        ordered = ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-rc.2", "1.0.0-rc.10", "1.0.0", "1.0.1"]  # fmt: skip
        shuffled = ["1.0.1", "1.0.0-rc.10", "1.0.0-alpha", "1.0.0", "1.0.0-rc.2", "1.0.0-alpha.1"]  # fmt: skip
        self.assertEqual(sorted(shuffled, key=release.precedence), ordered)


class Prepare(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = tmp.name

    def files(self, version, changelog=CHANGELOG):
        for name, text in ((release.VERSION_FILE, CARGO.format(version)),
                           (release.LOCK_FILE, LOCK.format(version)),
                           ("CHANGELOG.md", changelog)):  # fmt: skip
            release.write(self.root, name, text)

    def read(self, name):
        with open(os.path.join(self.root, name), encoding="utf-8") as f:
            return f.read()

    def test_final_release(self):
        self.files("1.0.0")
        self.assertEqual(release.prepare(self.root, "minor"), ("1.0.0", "1.1.0"))
        self.assertEqual(release.current_version(self.root), "1.1.0")
        text = self.read("CHANGELOG.md")
        # A new [Unreleased] with the hints for the next release
        self.assertRegex(
            text,
            r"## \[Unreleased\]\n\n<!--[^>]*-->\n\n## \[1\.1\.0\] - \d{4}-\d\d-\d\d\n",
        )
        links = re.findall(r"^\[[^\]]+\]: .*$", text, re.M)
        self.assertEqual(links, [
            "[Unreleased]: https://github.com/o/r/compare/v1.1.0...HEAD",
            "[1.1.0]: https://github.com/o/r/compare/v1.0.0...v1.1.0",
            "[1.0.0]: https://github.com/o/r/releases/tag/v1.0.0",
        ])  # fmt: skip
        self.assertEqual(release.notes(self.root, "1.1.0"), "### Fixed\n\n- A fix.")
        self.assertIn("comment", self.read(release.VERSION_FILE))  # the rest of the line stays
        self.assertIn('version = "0.62.2"', self.read(release.VERSION_FILE))  # not a dependency's
        self.assertIn('name = "aaa"\nversion = "1.0.0"', self.read(release.LOCK_FILE))
        self.assertIn('name = "app"\nversion = "1.1.0"', self.read(release.LOCK_FILE))

    def test_a_pre_release_keeps_its_lines_under_unreleased(self):
        self.files("1.0.0")
        release.prepare(self.root, "1.1.0-rc.1")
        self.assertEqual(self.read("CHANGELOG.md"), CHANGELOG)
        self.assertEqual(release.notes(self.root, "1.1.0-rc.1"), "### Fixed\n\n- A fix.")

    def test_the_bump_fits_the_notes(self):
        for version, last, heading, warned in (
            ("1.0.1", "1.0.0", "🐛 Fixes", False),
            ("1.0.1", "1.0.0", "🚀 Features", True),
            ("1.1.0", "1.0.0", "🚀 Features", False),
            ("1.1.0", "1.0.0", "⚠️ Breaking changes", True),
            ("2.0.0", "1.0.0", "⚠️ Breaking changes", False),
            ("0.2.0", "0.1.0", "⚠️ Breaking changes", False),  # before 1.0: minor
            ("0.1.1", "0.1.0", "⚠️ Breaking changes", True),
            ("1.1.0-rc.1", "1.0.0", "⚠️ Breaking changes", False),  # candidates: not checked
        ):
            with self.subTest(version=version, last=last, heading=heading):
                text = CHANGELOG.replace("1.0.0", last).replace("### Fixed", f"### {heading}")
                if "-" not in version:
                    text = text.replace(
                        "## [Unreleased]", f"## [Unreleased]\n\n## [{version}] - 2026-11-01"
                    )
                self.files(version, text)
                warning = release.check_bump(self.root, version)
                self.assertEqual(bool(warning), warned, warning)
        self.files("0.1.0", CHANGELOG.replace("## [1.0.0] - 2026-10-01", "## [0.1.0] - 2026-10-01"))
        self.assertEqual(release.check_bump(self.root, "0.1.0"), "")  # the first release

    def test_nothing_to_release(self):
        self.files("1.0.0", CHANGELOG.replace("### Fixed\n\n- A fix.\n\n", ""))
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            release.prepare(self.root, "patch")

    def test_notes_for_an_unknown_version(self):
        self.files("1.0.0")
        with self.assertRaises(SystemExit):
            release.notes(self.root, "3.0.0")

    def test_the_real_files_work(self):
        for name in (release.VERSION_FILE, "CHANGELOG.md"):
            release.write(self.root, name, release.read(ROOT, name))
        version = release.current_version(self.root)
        text = release.read(self.root, "CHANGELOG.md")
        if f"## [{version}]" in text:
            self.assertTrue(release.notes(self.root, version))
        else:  # before the first release
            self.assertTrue(release.section(text, "Unreleased"))
        self.assertTrue(release.repo_url(self.root).startswith("https://github.com/"))

    def test_the_first_release_may_keep_the_version(self):
        self.assertEqual(release.bump("0.1.0", "0.1.0", first=True), "0.1.0")
        self.assertEqual(release.bump("0.1.0", "1.0.0", first=True), "1.0.0")
        with self.assertRaises(SystemExit):
            release.bump("0.1.0", "0.1.0")


if __name__ == "__main__":
    unittest.main()
