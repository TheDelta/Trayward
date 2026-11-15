// Commit messages and pull request titles (the squash commit on main).
// The scopes are the parts of the app, plus the ones the changelog sorts by.
export const SCOPES = [
  "ci",
  "demo",
  "deps-dev",
  "deps",
  "flyout",
  "hooks",
  "i18n",
  "notifications",
  "release",
  "security",
  "sessions",
  "settings",
  "stats",
  "usage",
  "widget",
];

export default {
  extends: ["@commitlint/config-conventional"],
  rules: {
    "header-max-length": [2, "always", 100], // Dependabot's titles are long
    "scope-enum": [2, "always", SCOPES],
  },
};
