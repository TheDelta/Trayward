"""Tests of scripts/licenses.py: which crates count, and how their texts are put together."""

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import licenses


def package(name, folder, version="1.0.0"):
    return {
        "id": f"{name} {version}",
        "name": name,
        "version": version,
        "license": "MIT",
        "repository": f"https://example.com/{name}",
        "manifest_path": os.path.join(folder, name, "Cargo.toml"),
    }


def dep(package_id, *kinds):
    return {"pkg": package_id, "dep_kinds": [{"kind": kind} for kind in kinds]}


class Crates(unittest.TestCase):
    def test_takes_what_the_exe_is_built_from(self):
        names = [
            "trayward",
            "trayward-core",
            "used",
            "deeper",
            "builder",
            "tests-only",
            "other-apps",
        ]
        meta = {
            "packages": [package(name, "x") for name in names],
            "workspace_members": ["trayward 1.0.0", "trayward-core 1.0.0"],
            "resolve": {
                "nodes": [
                    {
                        "id": "trayward 1.0.0",
                        "deps": [
                            dep("trayward-core 1.0.0", None),
                            dep("used 1.0.0", None),
                            dep("builder 1.0.0", "build"),
                            dep("tests-only 1.0.0", "dev"),
                        ],
                    },
                    # The workspace's own crate isn't listed, what it uses is
                    {"id": "trayward-core 1.0.0", "deps": [dep("deeper 1.0.0", None, "dev")]},
                    {"id": "used 1.0.0", "deps": []},
                    {"id": "deeper 1.0.0", "deps": []},
                    {"id": "builder 1.0.0", "deps": []},
                    {"id": "tests-only 1.0.0", "deps": [dep("other-apps 1.0.0", None)]},
                    {"id": "other-apps 1.0.0", "deps": []},
                ]
            },
        }
        self.assertEqual([p["name"] for p in licenses.crates(meta)], ["builder", "deeper", "used"])


class Texts(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def crate(self, name, files):
        os.makedirs(os.path.join(self.dir.name, name))
        for file, text in files.items():
            with open(
                os.path.join(self.dir.name, name, file), "w", encoding="utf-8", newline=""
            ) as f:
                f.write(text)
        return package(name, self.dir.name)

    def test_reads_license_files_however_they_are_named(self):
        crate = self.crate(
            "a",
            {
                "LICENSE-MIT": "MIT text  \r\nsecond line\r\n\r\n",
                "license-apache-2.0": "Apache text\n",
                "COPYING": "Copying\n",
                "Cargo.toml": "",
                "README.md": "not a license",
                "licenses.rs": "code",
            },
        )
        self.assertEqual(
            licenses.texts(crate),
            [
                ("COPYING", "Copying\n"),
                ("license-apache-2.0", "Apache text\n"),
                ("LICENSE-MIT", "MIT text\nsecond line\n"),
            ],
        )

    def test_a_crate_without_a_file_has_none(self):
        self.assertEqual(licenses.texts(self.crate("bare", {"Cargo.toml": ""})), [])

    def test_the_same_text_is_printed_once(self):
        a = self.crate("a", {"LICENSE-APACHE": "Apache text\r\n", "LICENSE-MIT": "MIT, a's\n"})
        b = self.crate("b", {"LICENSE-APACHE": "Apache text\n"})
        text = licenses.render([(a, licenses.texts(a)), (b, licenses.texts(b))])
        self.assertIn("a 1.0.0: MIT, https://example.com/a\n", text)
        self.assertEqual(text.count("Apache text"), 1)
        self.assertIn("From: a 1.0.0, b 1.0.0\n", text)
        self.assertIn("From: a 1.0.0\n", text)
        self.assertNotIn("\r", text)
        self.assertTrue(text.endswith("MIT, a's\n"))


if __name__ == "__main__":
    unittest.main()
