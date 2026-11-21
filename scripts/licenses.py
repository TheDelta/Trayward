#!/usr/bin/env python3
"""The licenses of the Rust crates trayward.exe is built from, as one text file.

    python scripts/licenses.py                 # checks that every crate has its license text
    python scripts/licenses.py --out FILE      # writes THIRD-PARTY-LICENSES.txt for a release

The release workflow attaches the file to the release, next to the exe; the README and
the About page link to it. The crates are the ones `Cargo.lock` pins for the Windows
exe (not the ones only tests use), so the file says the same wherever it's made. Their
license texts are read from the crates' own sources, which cargo has downloaded.

A crate without a license file fails the run: add its text to `EXTRA` below.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# The published exe's platform, whatever system this runs on
TARGET = "x86_64-pc-windows-msvc"
# The package the exe is built from, and the workspace's own crates (Trayward's license)
APP = "trayward"
# License, copying and notice files as crates name them (LICENSE-MIT, license-apache-2.0, COPYING…)
LICENSE_FILE = re.compile(
    r"^(license|licence|copying|notice|unlicense|copyright)([-_.].*)?$", re.IGNORECASE
)
# License texts of crates that don't ship one: {crate name: text}
EXTRA: dict[str, str] = {}

HEADER = """\
Third-party licenses
====================

Trayward itself is under the MIT license (LICENSE in its repository). trayward.exe is
built from the Rust crates listed below and from Rust's standard library; this file has
their licenses. A crate under "A OR B" is used under either one, and both texts are here.

Rust's standard library (MIT OR Apache-2.0, Copyright (c) The Rust Project
Contributors, https://github.com/rust-lang/rust) is used under the MIT and Apache-2.0
licenses, whose texts are among those below.
"""


def metadata(root):
    """`cargo metadata` for the Windows exe, with the versions Cargo.lock pins."""
    out = subprocess.run(  # noqa: S603  # fixed arguments, no shell
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET],  # noqa: S607  # cargo from PATH
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if out.returncode != 0:
        sys.exit(f"cargo metadata failed:\n{out.stderr}")
    return json.loads(out.stdout)


def crates(meta, app=APP):
    """The third-party packages the app is built from: what it depends on and what
    builds it (build scripts, macros), not what only its tests use. Sorted by name."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    own = set(meta["workspace_members"])
    start = next(i for i, p in packages.items() if p["name"] == app and i in own)
    seen, todo = {start}, [start]
    while todo:
        for dep in nodes[todo.pop()]["deps"]:
            used = any(kind.get("kind") in (None, "build") for kind in dep["dep_kinds"])
            if used and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                todo.append(dep["pkg"])
    found = [packages[i] for i in seen if i not in own]
    return sorted(found, key=lambda p: (p["name"], p["version"]))


def clean(text):
    """A license text with LF line ends and no spaces at their ends, so that the same
    license from two crates is the same text."""
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    return "\n".join(lines).strip("\n") + "\n"


def texts(package):
    """The license texts a crate ships, as (file name, text), sorted by name."""
    folder = os.path.dirname(package["manifest_path"])
    found = []
    for name in sorted(os.listdir(folder), key=str.lower):
        path = os.path.join(folder, name)
        if LICENSE_FILE.match(name) and os.path.isfile(path):
            with open(path, encoding="utf-8", errors="replace") as f:
                found.append((name, clean(f.read())))
    if not found and package["name"] in EXTRA:
        found.append(("LICENSE", clean(EXTRA[package["name"]])))
    return found


def render(found):
    """The file: the crates with their licenses, then every different text once, with
    the crates it came from. `found` is [(package, [(file name, text)])]."""
    out = [HEADER, "", "Crates", "------", ""]
    for package, _ in found:
        where = package.get("repository") or f"https://crates.io/crates/{package['name']}"
        out.append(
            f"{package['name']} {package['version']}: {package.get('license') or 'see its text'}, {where}"
        )
    by_text = {}
    for package, files in found:
        for _, text in files:
            key = hashlib.sha256(text.encode()).hexdigest()
            by_text.setdefault(key, (text, []))[1].append(f"{package['name']} {package['version']}")
    out += ["", "", "License texts", "-------------"]
    for text, users in by_text.values():
        out += ["", "=" * 78, "From: " + ", ".join(users), "=" * 78, "", text.rstrip("\n")]
    return "\n".join(out) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--root", default=ROOT, help="the repository (default: this one)")
    parser.add_argument("--out", help="write the file here (default: only check)")
    args = parser.parse_args()

    found = [(p, texts(p)) for p in crates(metadata(args.root))]
    missing = [f"{p['name']} {p['version']}" for p, files in found if not files]
    if missing:
        sys.exit(
            "No license text found for: "
            + ", ".join(missing)
            + " (add it to EXTRA in scripts/licenses.py)"
        )
    if args.out:
        with open(args.out, "w", encoding="utf-8", newline="\n") as f:
            f.write(render(found))
    print(
        f"{len(found)} crates, each with its license text" + (f": {args.out}" if args.out else "")
    )


if __name__ == "__main__":
    main()
