// Pre-commit: the `npm run check` tools on the staged files only
import { pyTool } from "./scripts/tools.mjs";

const q = (s) => `"${s}"`;
const ruff = q(pyTool("ruff"));
const shellcheck = q(pyTool("shellcheck"));
const files = (list) => list.map(q).join(" ");

export default {
  // rustfmt formats whole crates; Clippy runs before the push (pre-push), it's slow
  "*.rs": () => ["cargo fmt"],
  "*.{mjs,json,yaml}": ["prettier --write --ignore-unknown"],
  "*.md": ["prettier --write --ignore-unknown", "markdownlint-cli2 --fix"],
  "*.py": [`${ruff} check --fix`, `${ruff} format`],
  ".github/workflows/*.yaml": (list) => [
    `${q(pyTool("actionlint"))} -shellcheck ${shellcheck} -pyflakes= ${files(list)}`,
    `${q(pyTool("zizmor"))} --offline ${files(list)}`,
  ],
  "*": ["cspell --no-progress --no-must-find-files --gitignore"],
};
