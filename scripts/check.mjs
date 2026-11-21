// Every check CI runs; lists the failed ones at the end.
//   node scripts/check.mjs [--fix] [--no-tests] [--no-rust]
// --no-rust: leave out rustfmt, Clippy, the Rust docs, cargo-deny, the crates' licenses and the Rust
// tests (CI runs them
// in their own jobs).
import { spawnSync } from "node:child_process";
import { nodeTool, pyTool, root } from "./tools.mjs";

const fix = process.argv.includes("--fix");
const tests = !process.argv.includes("--no-tests");
const rust = !process.argv.includes("--no-rust");
const ci = Boolean(process.env.GITHUB_ACTIONS);
// For zizmor's online audits
const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;

const node = (pkg, ...args) => [...nodeTool(pkg), ...args];
const py = (tool, ...args) => [pyTool(tool), ...args];
const python = process.platform === "win32" ? "python" : "python3";

// [name, check command, fix command (optional)]
const steps = [
  ...(rust
    ? [
        ["rustfmt (format)", ["cargo", "fmt", "--check"], ["cargo", "fmt"]],
        ["Clippy (lint)", ["cargo", "clippy", "--all-targets", "--locked", "--", "-D", "warnings"]],
        // Broken links and HTML in doc comments (the rustdoc lints in Cargo.toml deny them)
        [
          "Rust docs",
          ["cargo", "doc", "--no-deps", "--document-private-items", "--workspace", "--locked"],
        ],
        // Known vulnerabilities, licenses, sources of the crates (deny.toml)
        ["cargo-deny (dependencies)", ["cargo-deny", "--locked", "check"]],
        // Every crate in the exe ships its license text (the release's THIRD-PARTY-LICENSES.txt)
        ["Third-party licenses", [python, "scripts/licenses.py"]],
      ]
    : []),
  ["Prettier (format)", node("prettier", "--check", "."), node("prettier", "--write", ".")],
  ["markdownlint", node("markdownlint-cli2"), node("markdownlint-cli2", "--fix")],
  // --dot: .github and .vscode too, as the hook sees them
  ["CSpell (spelling)", node("cspell", "--no-progress", "--gitignore", "--dot", ".")],
  ["Ruff (lint)", py("ruff", "check", "."), py("ruff", "check", "--fix", ".")],
  ["Ruff (format)", py("ruff", "format", "--check", "."), py("ruff", "format", ".")],
  [
    "actionlint (workflows)",
    py("actionlint", "-shellcheck", pyTool("shellcheck"), "-pyflakes", ""),
  ],
  ["zizmor (workflow security)", py("zizmor", ...(token ? [] : ["--offline"]), ".github")],
  ["Line endings (LF)", null],
  ...(tests
    ? [
        ...(rust ? [["Rust tests", ["cargo", "test", "--locked"]]] : []),
        ["Release script tests", [python, "-m", "unittest", "discover", "-s", "scripts/tests"]],
      ]
    : []),
];

// How to get a missing program (see CONTRIBUTING.md)
function install(cmd) {
  const name = cmd
    .split(/[\\/]/)
    .pop()
    .replace(/\.exe$/i, "");
  if (["cargo", "rustc"].includes(name)) return "install Rust with rustup (https://rustup.rs)";
  if (name === "cargo-deny") return "install it with `cargo install --locked cargo-deny@0.21.0`";
  if (["python", "python3"].includes(name)) return "install Python 3.11 or newer";
  if (["ruff", "actionlint", "zizmor", "shellcheck"].includes(name))
    return "install the Python tools with `pip install --require-hashes -r requirements-dev.txt`";
  return "install the Node tools with `npm ci`";
}

// Files committed with CRLF (or mixed) line endings; .gitattributes wants LF
function lineEndings() {
  // NOSONAR: git from PATH, on the developer's own machine
  const out = spawnSync("git", ["ls-files", "--eol"], {
    cwd: root,
    encoding: "utf8",
  });
  if (out.status !== 0) return out.stderr || "git ls-files failed";
  const bad = out.stdout.split("\n").filter((l) => /^i\/(crlf|mixed)/.test(l));
  return bad.length ? `CRLF line endings in the index:\n${bad.join("\n")}` : "";
}

const failed = [];
for (const [name, check, fixCmd] of steps) {
  console.log(ci ? `::group::${name}` : `\n=== ${name}`);
  let ok;
  if (check === null) {
    const problem = lineEndings();
    if (problem) console.log(problem);
    ok = !problem;
  } else {
    const [cmd, ...args] = fix && fixCmd ? fixCmd : check;
    const run = spawnSync(cmd, args, { cwd: root, stdio: "inherit" });
    if (run.error?.code === "ENOENT") console.log(`${cmd} not found: ${install(cmd)}`);
    ok = run.status === 0;
  }
  if (ci) console.log("::endgroup::");
  if (!ok) {
    failed.push(name);
    if (ci) console.log(`::error::${name} failed`);
  }
}

console.log(
  failed.length ? `\nFailed: ${failed.join(", ")}` : `\nAll ${steps.length} checks passed.`,
);
process.exitCode = failed.length ? 1 : 0;
