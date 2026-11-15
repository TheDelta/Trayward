#!/usr/bin/env python3
"""Release helper for the release workflows (and locally).

    python scripts/release.py current            # the version
    python scripts/release.py next minor         # what "minor" would make of it
    python scripts/release.py prepare minor      # bump the version + CHANGELOG.md
    python scripts/release.py prepare rc         # 1.2.0-rc.1 -> 1.2.0-rc.2
    python scripts/release.py prepare 1.2.0-rc.1
    python scripts/release.py notes 1.2.0        # that version's CHANGELOG section
    python scripts/release.py notes --github 1.2.0   # as the release page shows it
    python scripts/release.py missing            # fails if the notes lack a merged PR
    python scripts/release.py check-bump         # warns if the notes need a bigger bump

`prepare` adds the pull requests since the last release to [Unreleased] and bumps the
version (in Cargo.toml and Cargo.lock); a final version also gets its own section and
compare link. Bumps work like npm's, except that release candidates start at rc.1.
The first release may keep the version Cargo.toml has.
"""

import argparse
import io
import os
import re
import sys
from datetime import UTC, datetime

import changelog  # scripts/changelog.py, next to this script

# x.y.z, then a pre-release like rc.1 (dot-separated identifiers)
NUMBER = r"(0|[1-9]\d*)"
SEMVER = re.compile(rf"{NUMBER}\.{NUMBER}\.{NUMBER}(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?")
# Cargo.toml's [package] table, up to the next table
PACKAGE = re.compile(r"^\[package\]\n(?:(?!\[).*\n?)*", re.M)
UNRELEASED = "## [Unreleased]"
VERSION_FILE, LOCK_FILE, CHANGELOG_MD = "Cargo.toml", "Cargo.lock", "CHANGELOG.md"
PRE = ("prepatch", "preminor", "premajor")


def parse(version):
    m = SEMVER.fullmatch(version)
    if not m:
        sys.exit(f"Not a version: {version!r} (expected e.g. 1.2.0 or 1.2.0-rc.1)")
    major, minor, patch, pre = m.groups()
    return int(major), int(minor), int(patch), pre


def precedence(version):
    """Sort key following SemVer: 1.2.0-rc.1 < 1.2.0-rc.2 < 1.2.0 < 1.2.1."""
    major, minor, patch, pre = parse(version)
    if pre is None:
        return (major, minor, patch, 1, ())
    parts = tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in pre.split("."))
    return (major, minor, patch, 0, parts)


def bump(current, how, first=False):
    """The version `how` makes of `current`. first: no release yet, so an exact
    version may also be the current one."""
    major, minor, patch, pre = parse(current)
    if how == "rc":  # the next candidate of the current one
        m = re.fullmatch(r"rc\.(\d+)", pre or "")
        if not m:
            sys.exit(
                f"rc: {current} isn't a release candidate (x.y.z-rc.N); start one "
                "with prepatch, preminor or premajor"
            )
        return f"{major}.{minor}.{patch}-rc.{int(m.group(1)) + 1}"
    if how in PRE:  # the first candidate of the next patch / minor / major
        return f"{bump_final(major, minor, patch, None, how[3:])}-rc.1"
    if how in ("patch", "minor", "major"):
        return bump_final(major, minor, patch, pre, how)
    newer = precedence(how) > precedence(current)
    if not (newer or (first and precedence(how) == precedence(current))):
        sys.exit(f"{how} isn't newer than the current version {current}")
    return how


def bump_final(major, minor, patch, pre, how):
    """patch / minor / major; from a pre-release, the version it leads to if
    that's as far as the bump goes (1.2.0-rc.1 minor: 1.2.0)."""
    if how == "major":
        done = pre and minor == 0 and patch == 0
        return f"{major}.0.0" if done else f"{major + 1}.0.0"
    if how == "minor":
        return f"{major}.{minor}.0" if pre and patch == 0 else f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{patch}" if pre else f"{major}.{minor}.{patch + 1}"


def read(root, name):
    with open(os.path.join(root, name), encoding="utf-8") as f:
        return f.read()


def write(root, name, text):
    os.makedirs(os.path.dirname(os.path.join(root, name)), exist_ok=True)
    with open(os.path.join(root, name), "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def package_field(text, field):
    """The match of `field = "value"` in Cargo.toml's [package] (the value is group
    1), not a dependency's."""
    table = PACKAGE.search(text)
    if not table:
        return None
    return re.compile(rf'^{field} = "([^"]*)"', re.M).search(text, table.start(), table.end())


def current_version(root):
    m = package_field(read(root, VERSION_FILE), "version")
    if not m:
        sys.exit(f'{VERSION_FILE} has no version = "x.y.z" in [package]')
    return m.group(1)


def repo_url(root):
    m = package_field(read(root, VERSION_FILE), "repository")
    return m.group(1).rstrip("/") if m else ""


def set_version(root, version):
    """The version in Cargo.toml's [package] and in Cargo.lock's entry for it."""
    text = read(root, VERSION_FILE)
    m = package_field(text, "version")
    write(root, VERSION_FILE, f"{text[: m.start(1)]}{version}{text[m.end(1) :]}")
    name = package_field(text, "name")
    if name and os.path.exists(os.path.join(root, LOCK_FILE)):
        lock = read(root, LOCK_FILE)
        entry = re.compile(
            rf'^(\[\[package\]\]\nname = "{re.escape(name.group(1))}"\nversion = ")[^"]*"', re.M
        )
        write(root, LOCK_FILE, entry.sub(lambda e: f'{e.group(1)}{version}"', lock, count=1))


def released(root):
    """Whether a version was released (tagged) yet; outside a git repository, yes."""
    if not os.path.exists(os.path.join(root, ".git")):
        return True
    return bool(changelog.git(root, "tag", "--list", "v[0-9]*").strip())


def find_section(text, name):
    """The match of '## [name] ...' with its body (group 1) up to the next
    '## [' or the link list."""
    return re.search(
        rf"^## \[{re.escape(name)}\][^\n]*\n(.*?)(?=^## \[|^\[[^\]]+\]: |\Z)",
        text,
        re.S | re.M,
    )


def section(text, name):
    """The body of a version's section, without HTML comments (the hints in
    [Unreleased])."""
    m = find_section(text, name)
    return changelog.without_comments(m.group(1)) if m else None


def require_section(text, name):
    m = find_section(text, name)
    if not m:
        sys.exit(f"CHANGELOG.md has no section [{name}]")
    return m


def replace_section(text, name, body):
    m = require_section(text, name)
    rest = text[m.end(1) :]
    gap = "\n\n" if rest else "\n"  # a blank line before what follows
    return f"{text[: m.start(1)]}\n{body}{gap}{rest}"


def tidy(text):
    """One blank line between blocks and one newline at the end, as markdownlint
    and Prettier want them (MD012, MD047)."""
    return re.sub(r"\n{3,}", "\n\n", text).rstrip("\n") + "\n"


def notes(root, version, github=False):
    """A version's notes, unwrapped for GitHub. github: @mentions and #refs, which
    GitHub links itself, and the compare link."""
    text = read(root, CHANGELOG_MD)
    body = release_body(text, version)
    if not body:
        sys.exit(f"CHANGELOG.md has no notes for [{version}]")
    body = changelog.unwrap(body)
    if not github:
        return body
    link = re.search(rf"^\[{re.escape(version)}\]: (\S+)$", text, re.M)
    compare = link.group(1) if link and "/compare/" in link.group(1) else None
    full = f"\n\n**Full Changelog**: {compare}" if compare else ""
    return changelog.for_github(body) + full


def release_body(text, version):
    """What a version's notes are made of: its section, or [Unreleased] for a
    pre-release (candidates collect their lines there)."""
    body = section(text, version)
    if body is None and "-" in version:
        body = section(text, "Unreleased")
    return body or ""


def missing(root, version):
    """Changelog lines of pull requests merged after Prepare release made the
    release commit of `version` and that its notes don't have (lines removed from
    the notes by hand before that stay removed). Offline, so without authors."""
    found = changelog.commits(root)
    title = f"v{version}"
    made = [
        c["sha"]
        for c in found
        if changelog.is_release(p := changelog.parse(c)) and p["text"] == title
    ]
    if not made:
        return []  # not a release branch
    late = changelog.commits(root, since=made[-1])
    body = release_body(read(root, CHANGELOG_MD), version)
    url = f"https://github.com/{github_repo(root)}"
    return changelog.lines(late, body, url, lambda _: {})


def bump_kind(old, new):
    o, n = parse(old)[:3], parse(new)[:3]
    return "major" if n[0] > o[0] else "minor" if n[:2] > o[:2] else "patch"


def check_bump(root, version):
    """A warning (Markdown) when the notes ask for a bigger bump than the one from
    the last final release, or "". Breaking changes need a major version (a minor
    one before 1.0), features at least a minor one."""
    if "-" in version:
        return ""
    text = read(root, CHANGELOG_MD)
    finals = [v for v in released_versions(text) if v != version and "-" not in v]
    if not finals:
        return ""
    last = max(finals, key=precedence)
    kind = bump_kind(last, version)
    body = release_body(text, version)
    headings = set(re.findall(r"^### (.+)$", body, re.M))
    breaking = "major" if parse(last)[0] > 0 else "minor"
    need = (
        breaking
        if changelog.BREAKING in headings
        else "minor"
        if changelog.FEATURES in headings
        else "patch"
    )
    order = ["patch", "minor", "major"]
    if order.index(kind) >= order.index(need):
        return ""
    why = "breaking changes" if changelog.BREAKING in headings else "new features"
    return (
        f"> [!WARNING]\n> **{last} → {version} is a {kind} release, but the notes list "
        f"{why}:** that asks for a {need} release. Run Prepare release again with "
        f"`{need}`, or move the lines if they're in the wrong section.\n"
    )


def released_versions(text):
    return re.findall(r"^## \[([^\]]+)\] - ", text, re.M)


def update_links(text, version, url):
    """Keep a Changelog's compare links: [Unreleased] and one per version."""
    if not url:
        return text
    previous = released_versions(text)
    previous = [v for v in previous if v != version]
    lines = [f"[Unreleased]: {url}/compare/v{version}...HEAD"]
    lines.append(
        f"[{version}]: {url}/compare/v{previous[0]}...v{version}"
        if previous
        else f"[{version}]: {url}/releases/tag/v{version}"
    )
    text = re.sub(r"^\[Unreleased\]: .*\n?", "", text, flags=re.M).rstrip("\n")
    links = re.search(r"(?:^\[[^\]]+\]: .*\n)+\Z", text + "\n", re.M)  # each line ends in \n
    if links:  # the newest version's link goes first
        head, tail = text[: links.start()].rstrip("\n"), links.group(0).rstrip("\n")
        return f"{head}\n\n" + "\n".join(lines) + f"\n{tail}\n"
    return f"{text}\n\n" + "\n".join(lines) + "\n"


def github_repo(root):
    """owner/name for the links: the repository the workflow runs in, else the
    one PROJECT_URL names."""
    repo = os.environ.get("GITHUB_REPOSITORY")
    return repo or repo_url(root).removeprefix("https://github.com/")


def add_changes(root, text, repo, lookup):
    """[Unreleased] with a line for each pull request (or commit) since the last
    release that it doesn't mention yet."""
    body = require_section(text, "Unreleased").group(1)
    entries = changelog.lines(changelog.commits(root), body, f"https://github.com/{repo}", lookup)
    print(f"{len(entries)} changes added to [Unreleased]", file=sys.stderr)
    if not entries:
        return text
    return replace_section(text, "Unreleased", changelog.add(body.strip(), entries))


def prepare(root, how, repo=None, lookup=None):
    """Bump the version and CHANGELOG.md. repo (owner/name) and lookup (pull request
    details, see changelog.github_lookup) default to the workflow's."""
    current = current_version(root)
    version = bump(current, how, first=not released(root))
    text = read(root, CHANGELOG_MD)
    if text.count(UNRELEASED) != 1:
        sys.exit(f"CHANGELOG.md needs exactly one '{UNRELEASED}' heading")
    repo = repo or github_repo(root)
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    text = add_changes(root, text, repo, lookup or changelog.github_lookup(repo, token))
    if "-" not in version:
        text = release_section(text, version)
        text = update_links(text, version, repo_url(root))
    set_version(root, version)
    write(root, CHANGELOG_MD, tidy(text))
    return current, version


def release_section(text, version):
    """[Unreleased] becomes '[version] - today' (without the hints), under a new
    [Unreleased] that starts with them."""
    body = section(text, "Unreleased")
    if not body:
        sys.exit("CHANGELOG.md: nothing under [Unreleased] to release")
    if f"## [{version}]" in text:
        sys.exit(f"CHANGELOG.md already has a section for [{version}]")
    today = datetime.now(UTC).date().isoformat()
    text = replace_section(text, "Unreleased", body)
    return text.replace(
        UNRELEASED,
        f"{UNRELEASED}\n\n{changelog.TEMPLATE}\n\n## [{version}] - {today}",
        1,
    )


def main():
    p = argparse.ArgumentParser(description="Trayward release helper")
    p.add_argument("--root", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("current", help="print the version")
    sub.add_parser("next", help="print the version a bump would give").add_argument("how")
    sub.add_parser("prepare", help="bump the version and CHANGELOG.md").add_argument("how")
    notes_cmd = sub.add_parser("notes", help="print a version's CHANGELOG section")
    notes_cmd.add_argument("version")
    notes_cmd.add_argument(
        "--github", action="store_true", help="as the GitHub release page shows them"
    )
    sub.add_parser(
        "missing", help="fail if a pull request since the last release isn't in the notes"
    )
    sub.add_parser("check-bump", help="warn if the notes ask for a bigger version bump")
    a = p.parse_args()
    if isinstance(sys.stdout, io.TextIOWrapper):  # the notes have arrows and stars
        sys.stdout.reconfigure(encoding="utf-8")
    if a.cmd == "current":
        print(current_version(a.root))
    elif a.cmd == "next":
        print(bump(current_version(a.root), a.how, first=not released(a.root)))
    elif a.cmd == "prepare":
        old, new = prepare(a.root, a.how)
        print(f"{old} -> {new}", file=sys.stderr)
        print(new)
    elif a.cmd == "missing":
        version = current_version(a.root)
        lost = missing(a.root, version)
        if lost:
            print(f"Merged after Prepare release, so not in the notes of {version}:")
            print("\n".join(text for _, text in lost))
            sys.exit(1)
        print(f"The notes of {version} have every pull request since the last release.")
    elif a.cmd == "check-bump":
        print(check_bump(a.root, current_version(a.root)), end="")
    else:
        print(notes(a.root, a.version, a.github))


if __name__ == "__main__":
    main()
