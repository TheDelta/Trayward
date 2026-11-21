//! Merge requests, pull requests and pipelines from GitLab and GitHub: the
//! one on each session's branch, and the user's own open ones in the
//! sessions' projects.
//!
//! Through the hosts' own command-line tools, `glab` and `gh`: with Trayward's
//! own token for the host when the user pasted one (kept in the system's
//! credential store, handed to the tool as an environment variable, never on
//! its command line), else with the tool's own login. One GraphQL request per
//! host asks for everything at once. Off unless switched on (Settings →
//! Extensions), since it contacts GitLab and GitHub.
//!
//! A session's repository and branch come from its folder's `.git`: the
//! `origin` remote names the host and project, `HEAD` the branch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How often everything is asked for again.
const EVERY: Duration = Duration::from_secs(60);
/// A tool that takes longer is stopped.
const TIMEOUT: Duration = Duration::from_secs(30);
/// How many of the user's own open ones are asked for per host, before only
/// those in the sessions' projects are kept.
const MINE: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Forge {
    GitLab,
    GitHub,
}

/// A project on a host, e.g. GitLab `gitlab.example.com` `group/project`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Repo {
    pub forge: Forge,
    pub host: String,
    pub path: String,
}

/// A pipeline's (or the checks') state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checks {
    Running,
    Passed,
    Failed,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Open,
    Merged,
    Closed,
}

/// A merge request (GitLab) or pull request (GitHub).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub forge: Forge,
    pub number: u64,
    pub title: String,
    pub url: String,
    /// The project, e.g. `group/project`.
    pub project: String,
    pub state: State,
    pub draft: bool,
    pub checks: Option<Checks>,
    /// Last updated (unix seconds; 0 if unknown).
    pub updated: i64,
}

impl Change {
    /// `!37` on GitLab, `#37` on GitHub.
    pub fn reference(&self) -> String {
        match self.forge {
            Forge::GitLab => format!("!{}", self.number),
            Forge::GitHub => format!("#{}", self.number),
        }
    }
}

/// What a session's branch has: its newest merge or pull request, and the
/// branch's latest pipeline when there's none (GitLab).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub change: Option<Change>,
    pub checks: Option<Checks>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Per session folder.
    pub sessions: HashMap<String, Branch>,
    /// The user's own open merge and pull requests in the sessions' projects,
    /// newest first.
    pub mine: Vec<Change>,
    /// How `glab` and `gh` stand, for the settings.
    pub tools: Tools,
    /// How many pokes there had been when this was asked ([`spawn`]): an answer to
    /// the poke for a pasted or removed token, or a later one, shows what it did.
    pub answers: u64,
}

/// How one of the two tools stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tool {
    /// It's in PATH.
    pub found: bool,
    /// Where (looked up, also while switched off).
    pub path: Option<String>,
    /// What it says its version is, e.g. `glab 1.52.0` (asked while switched on).
    pub version: Option<String>,
    /// The hosts it was needed for (the ones the sessions' projects are on, the
    /// ones Trayward keeps a token for, and for `gh` github.com), sorted.
    pub hosts: Vec<Host>,
}

/// A host a tool was asked about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Host {
    pub name: String,
    /// The tool has a working login there: with Trayward's token when it keeps
    /// one, else the tool's own.
    pub ok: bool,
    /// Trayward keeps its own token for it.
    pub token: bool,
    /// When that token expires (unix seconds), when the host says.
    pub expires: Option<i64>,
}

/// How `glab` and `gh` stand. While merge and pull requests are switched off,
/// neither is run: only whether they're found is known then.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tools {
    pub glab: Tool,
    pub gh: Tool,
    /// They were asked for their logins (merge and pull requests are on).
    pub asked: bool,
}

impl Tools {
    /// Whether the two are in PATH: looked up, not run.
    fn found() -> Tools {
        let found = |program: &str| {
            let path = crate::os::find_program(program);
            Tool { found: path.is_some(), path: path.map(|p| p.display().to_string()), ..Default::default() }
        };
        Tools { glab: found("glab"), gh: found("gh"), asked: false }
    }
}

// ---------- Trayward's own tokens ----------

/// The tool that talks to a forge.
pub fn program(forge: Forge) -> &'static str {
    match forge {
        Forge::GitLab => "glab",
        Forge::GitHub => "gh",
    }
}

/// Where Trayward keeps its own token for `host` in the credential store, e.g.
/// `trayward:gitlab:gitlab.example.com`.
pub fn token_target(forge: Forge, host: &str) -> String {
    let kind = match forge {
        Forge::GitLab => "gitlab",
        Forge::GitHub => "github",
    };
    format!("{TOKEN_PREFIX}{kind}:{}", host.to_ascii_lowercase())
}

/// What the names of Trayward's entries in the credential store start with.
const TOKEN_PREFIX: &str = "trayward:";

/// The forge and host of an entry's name ([`token_target`]).
fn token_host(target: &str) -> Option<(Forge, String)> {
    let (kind, host) = target.strip_prefix(TOKEN_PREFIX)?.split_once(':')?;
    let forge = match kind {
        "gitlab" => Forge::GitLab,
        "github" => Forge::GitHub,
        _ => return None,
    };
    (!host.is_empty()).then(|| (forge, host.to_string()))
}

/// The hosts Trayward keeps a token for. None in the demo.
fn token_hosts() -> Vec<(Forge, String)> {
    if crate::demo::active() {
        return Vec::new();
    }
    crate::os::secret::list(TOKEN_PREFIX).iter().filter_map(|t| token_host(t)).collect()
}

/// Trayward's own token for `host`, if it keeps one. Never in the demo.
fn stored_token(forge: Forge, host: &str) -> Option<String> {
    if crate::demo::active() {
        return None;
    }
    crate::os::secret::read(&token_target(forge, host)).filter(|t| !t.is_empty())
}

/// Keeps `token` as Trayward's own for `host` (replacing one kept before).
pub fn save_token(forge: Forge, host: &str, token: &str) -> Result<(), String> {
    let user = format!("Trayward: {} for {host}", program(forge));
    crate::os::secret::write(&token_target(forge, host), &user, token).map_err(|e| redact(&e, Some(token)))
}

/// Forgets Trayward's own token for `host`: the tool's own login is used again.
pub fn remove_token(forge: Forge, host: &str) -> Result<(), String> {
    crate::os::secret::delete(&token_target(forge, host))
}

/// Whether pasted text looks like a token: one word of 20 to 255 visible ASCII
/// characters (not a sentence, a path or a URL that was on the clipboard).
pub fn looks_like_token(text: &str) -> bool {
    (20..=255).contains(&text.len())
        && text.bytes().all(|b| b.is_ascii_graphic())
        && !text.contains("://")
        && !text.contains('\\')
}

/// What tokens of GitLab and GitHub start with.
const TOKEN_STARTS: &[&str] =
    &["glpat-", "gloas-", "glptt-", "gldt-", "github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];

/// `text` with `token` and anything that looks like a GitLab or GitHub token
/// replaced by `[token]`: for messages that are shown or kept.
pub fn redact(text: &str, token: Option<&str>) -> String {
    let mut text = text.to_string();
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        text = text.replace(t, "[token]");
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let next = TOKEN_STARTS.iter().filter_map(|s| rest.find(s)).min();
        let Some(at) = next else {
            out.push_str(rest);
            break;
        };
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let end = from.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).unwrap_or(from.len());
        out.push_str("[token]");
        rest = from.get(end..).unwrap_or_default();
    }
    out
}

/// The environment variables that hand `token` to the tool for `host`.
fn token_env(forge: Forge, host: &str, token: &str) -> Vec<(&'static str, String)> {
    match forge {
        Forge::GitLab => vec![("GITLAB_TOKEN", token.to_string()), ("GITLAB_HOST", host.to_string())],
        Forge::GitHub if host.eq_ignore_ascii_case(GITHUB) => vec![("GH_TOKEN", token.to_string())],
        Forge::GitHub => vec![("GH_ENTERPRISE_TOKEN", token.to_string()), ("GH_HOST", host.to_string())],
    }
}

/// Runs `glab` or `gh` for `host`, with `token` (Trayward's own for it) when
/// given, else with the tool's own login. Returns what it wrote, or why it
/// failed (never with a token in it).
fn run_tool(
    forge: Forge,
    host: &str,
    args: &[&str],
    timeout: Duration,
    token: Option<&str>,
) -> Result<crate::os::Output, String> {
    let name = program(forge);
    let exe = crate::os::find_program(name).ok_or_else(|| format!("{name}: program not found"))?;
    let env = token.map(|t| token_env(forge, host, t)).unwrap_or_default();
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    crate::os::run_env(exe.as_os_str(), args, &env, timeout).map_err(|e| redact(&e, token))
}

/// Runs `glab` or `gh` for `host` with `token` (see [`run_tool`]); its output, or
/// why it failed.
fn answer_of(forge: Forge, host: &str, args: &[&str], token: Option<&str>) -> Result<String, String> {
    let name = program(forge);
    let out = run_tool(forge, host, args, TIMEOUT, token)?;
    if !out.complete {
        return Err(format!("{name}: no answer within {} s", TIMEOUT.as_secs()));
    }
    // GraphQL errors still come with a JSON answer; use what's there.
    let text = out.stdout_text();
    if text.trim().is_empty() {
        return Err(redact(out.stderr_text().trim(), token));
    }
    Ok(text)
}

/// Runs `glab` or `gh` for `host`, with Trayward's own token for it when it keeps
/// one, else with the tool's own login; its output, or why it failed.
pub(crate) fn run_for(forge: Forge, host: &str, args: &[&str]) -> Result<String, String> {
    answer_of(forge, host, args, stored_token(forge, host).as_deref())
}

/// Runs `glab` or `gh` with the tool's own login only, never with Trayward's
/// token (Copilot's monthly limit: the token is made for merge and pull requests).
pub(crate) fn run_with_own_login(forge: Forge, host: &str, args: &[&str]) -> Result<String, String> {
    answer_of(forge, host, args, None)
}

/// A date at the start of `s` (`2027-11-21`, as GitLab's `expires_at` and
/// GitHub's `github-authentication-token-expiration` begin), as unix seconds.
fn date_at_start(s: &str) -> Option<i64> {
    let day = s.trim().get(..10)?;
    crate::timefmt::parse_rfc3339(&format!("{day}T00:00:00Z"))
}

/// When GitHub says the token of a `gh api --include` answer expires.
fn github_expiry(answer: &str) -> Option<i64> {
    let header = "github-authentication-token-expiration:";
    answer
        .lines()
        .take_while(|l| !l.trim().is_empty())
        .find_map(|l| {
            l.get(..header.len()).filter(|h| h.eq_ignore_ascii_case(header)).and_then(|_| l.get(header.len()..))
        })
        .and_then(date_at_start)
}

/// When Trayward's `token` for `host` expires, as the host says.
fn token_expiry(forge: Forge, host: &str, token: &str) -> Option<i64> {
    let token = Some(token);
    match forge {
        Forge::GitLab => {
            let out = run_tool(
                forge,
                host,
                &["api", "--hostname", host, "personal_access_tokens/self"],
                LOGIN_TIMEOUT,
                token,
            );
            let v: Value = serde_json::from_slice(&out.ok().filter(|o| o.success)?.stdout).ok()?;
            date_at_start(v["expires_at"].as_str()?)
        }
        Forge::GitHub => {
            let out = run_tool(forge, host, &["api", "--hostname", host, "--include", "user"], LOGIN_TIMEOUT, token);
            github_expiry(&out.ok().filter(|o| o.success)?.stdout_text())
        }
    }
}

/// A token's fingerprint: a new token is a new login to check, without keeping the token.
fn fingerprint(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut h);
    h.finish()
}

// ---------- the repository of a folder ----------

/// The repository and branch of a folder (or a folder inside a repository).
pub fn repo_of(folder: &str) -> Option<(Repo, String)> {
    let (git_dir, common) = git_dirs(Path::new(folder))?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let branch = head.trim().strip_prefix("ref: refs/heads/")?.to_string();
    let config = std::fs::read_to_string(common.join("config")).ok()?;
    let url = origin_url(&config)?;
    Some((parse_remote(&url)?, branch))
}

/// The `.git` folder of a working tree and its common folder (they differ in
/// a `git worktree`, whose `.git` is a file pointing to its own folder).
fn git_dirs(start: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let dot = d.join(".git");
        if dot.is_dir() {
            return Some((dot.clone(), dot));
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let git_dir = d.join(text.trim().strip_prefix("gitdir:")?.trim());
            let common = std::fs::read_to_string(git_dir.join("commondir"))
                .map_or_else(|_| git_dir.clone(), |c| git_dir.join(c.trim()));
            return Some((git_dir, common));
        }
        dir = d.parent();
    }
    None
}

/// `[remote "origin"] url = …` from a git config.
fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
        } else if in_origin {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "url" {
                    return Some(value.trim().to_string());
                }
            }
        }
    }
    None
}

/// The host and project of a remote URL: `git@host:group/p.git`,
/// `ssh://git@host:2222/group/p.git`, `https://host/group/p`.
fn parse_remote(url: &str) -> Option<Repo> {
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?, path)
    } else {
        // scp-like: user@host:path
        let rest = url.rsplit_once('@').map_or(url, |(_, r)| r);
        rest.split_once(':')?
    };
    let host = host.to_ascii_lowercase();
    let path = path.trim_end_matches('/').trim_end_matches(".git").trim_matches('/').to_string();
    if host.is_empty() || !path.contains('/') {
        return None;
    }
    if not_gitlab(&host) {
        return None;
    }
    let forge = if host == "github.com" { Forge::GitHub } else { Forge::GitLab };
    Some(Repo { forge, host, path })
}

/// Hosts known not to be GitLab (any other host might be a GitLab of its own):
/// never asked, nor listed as a GitLab host in the settings.
fn not_gitlab(host: &str) -> bool {
    const OTHERS: &[&str] = &[
        "bitbucket.org",
        "codeberg.org",
        "gitea.com",
        "dev.azure.com",
        "ssh.dev.azure.com",
        "git.sr.ht",
        "sourceforge.net",
        "gitee.com",
        "huggingface.co",
        "hf.co",
    ];
    OTHERS.contains(&host) || [".visualstudio.com", ".sourceforge.net", ".sf.net"].iter().any(|s| host.ends_with(s))
}

// ---------- asking the hosts ----------

/// A GraphQL string literal.
fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

const GITLAB_MR: &str = "iid title webUrl state draft updatedAt project { fullPath } headPipeline { status }";
const GITHUB_PR: &str = "number title url state isDraft updatedAt repository { nameWithOwner } \
     commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }";

/// One query for a host: each branch (as `b0`, `b1`, …) and the user's own.
fn query(forge: Forge, branches: &[(String, String)]) -> String {
    let mut q = String::from("query {");
    for (i, (path, branch)) in branches.iter().enumerate() {
        match forge {
            Forge::GitLab => {
                q += &format!(
                    " b{i}: project(fullPath: {}) {{ mergeRequests(sourceBranches: [{}], first: 1, sort: UPDATED_DESC) \
                 {{ nodes {{ {GITLAB_MR} }} }} pipelines(ref: {}, first: 1) {{ nodes {{ status }} }} }}",
                    quoted(path),
                    quoted(branch),
                    quoted(branch)
                )
            }
            Forge::GitHub => {
                let (owner, name) = path.split_once('/').unwrap_or((path, ""));
                q += &format!(
                    " b{i}: repository(owner: {}, name: {}) {{ pullRequests(headRefName: {}, first: 1, \
                     orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ nodes {{ {GITHUB_PR} }} }} }}",
                    quoted(owner),
                    quoted(name),
                    quoted(branch)
                );
            }
        }
    }
    q += &match forge {
        Forge::GitLab => format!(
            " mine: currentUser {{ authoredMergeRequests(state: opened, first: {MINE}, sort: UPDATED_DESC) \
             {{ nodes {{ {GITLAB_MR} }} }} }} }}"
        ),
        Forge::GitHub => format!(
            " mine: viewer {{ pullRequests(states: OPEN, first: {MINE}, \
             orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ nodes {{ {GITHUB_PR} }} }} }} }}"
        ),
    };
    q
}

fn checks_of(status: &str) -> Option<Checks> {
    match status.to_ascii_uppercase().as_str() {
        "RUNNING" | "PENDING" | "CREATED" | "PREPARING" | "WAITING_FOR_RESOURCE" | "SCHEDULED" | "EXPECTED" => {
            Some(Checks::Running)
        }
        "SUCCESS" => Some(Checks::Passed),
        "FAILED" | "FAILURE" | "ERROR" => Some(Checks::Failed),
        "CANCELED" | "CANCELING" => Some(Checks::Canceled),
        _ => None, // skipped, manual: nothing to say
    }
}

fn change_of(forge: Forge, v: &Value) -> Option<Change> {
    let (number, url, project, draft, checks) = match forge {
        Forge::GitLab => (
            v["iid"].as_str()?.parse().ok()?,
            v["webUrl"].as_str()?,
            v["project"]["fullPath"].as_str().unwrap_or_default(),
            v["draft"].as_bool().unwrap_or(false),
            v["headPipeline"]["status"].as_str().and_then(checks_of),
        ),
        Forge::GitHub => (
            v["number"].as_u64()?,
            v["url"].as_str()?,
            v["repository"]["nameWithOwner"].as_str().unwrap_or_default(),
            v["isDraft"].as_bool().unwrap_or(false),
            v["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str().and_then(checks_of),
        ),
    };
    let state = match v["state"].as_str()?.to_ascii_lowercase().as_str() {
        "opened" | "open" => State::Open,
        "merged" => State::Merged,
        _ => State::Closed,
    };
    Some(Change {
        forge,
        number,
        title: v["title"].as_str().unwrap_or_default().to_string(),
        url: url.to_string(),
        project: project.to_string(),
        state,
        draft,
        checks,
        updated: v["updatedAt"].as_str().and_then(crate::timefmt::parse_rfc3339).unwrap_or(0),
    })
}

/// A branch's newest request and checks, if the host knows the project.
type BranchState = Option<(Option<Change>, Option<Checks>)>;

/// A host's answer: each branch's state (in the order asked) and the user's own.
fn parse(forge: Forge, answer: &Value, asked: usize) -> (Vec<BranchState>, Vec<Change>) {
    let data = &answer["data"];
    let list = |v: &Value| v["nodes"].as_array().cloned().unwrap_or_default();
    let branches = (0..asked)
        .map(|i| {
            let b = &data[format!("b{i}")];
            if b.is_null() {
                return None;
            }
            let (changes, pipeline) = match forge {
                Forge::GitLab => {
                    (list(&b["mergeRequests"]), b["pipelines"]["nodes"][0]["status"].as_str().and_then(checks_of))
                }
                Forge::GitHub => (list(&b["pullRequests"]), None),
            };
            let change = changes.first().and_then(|c| change_of(forge, c));
            let checks = change.as_ref().and_then(|c| c.checks).or(pipeline);
            Some((change, checks))
        })
        .collect();
    let mine_key = match forge {
        Forge::GitLab => "authoredMergeRequests",
        Forge::GitHub => "pullRequests",
    };
    let mine = list(&data["mine"][mine_key]).iter().filter_map(|c| change_of(forge, c)).collect();
    (branches, mine)
}

/// Which hosts `glab` and `gh` have a login for, as far as asked. Only the
/// GitLab hosts with one are asked for merge requests: a repository on any
/// other host (Bitbucket, Codeberg, a self-hosted git) isn't GitLab's business,
/// and `glab` would send it the query all the same. Each host is checked on its
/// own (`glab auth status --hostname`, `gh auth status --hostname`): a host
/// without a login can take glab half a minute to give up on, so it's checked
/// again only now and then.
#[derive(Default)]
struct Logins {
    /// Forge, host and the fingerprint of Trayward's token for it (0: none) →
    /// what was found out, and when.
    known: HashMap<(Forge, String, u64), (Login, Instant)>,
    /// A tool's path → what it says its version is.
    versions: HashMap<String, Option<String>>,
}

/// What a login check found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Login {
    ok: bool,
    token: bool,
    expires: Option<i64>,
}

/// How long a host without a (working) login isn't checked again.
const RECHECK_LOGIN: Duration = Duration::from_secs(30 * 60);
/// How long a working login isn't checked again (a token may have expired since).
const RECHECK_OK: Duration = Duration::from_secs(12 * 3600);
/// A login check that takes longer counts as no login.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(45);

impl Logins {
    /// Whether the forge's tool has a working login for `host` (Trayward's own
    /// token when it keeps one, else the tool's own login), and when that token
    /// expires.
    fn check(&mut self, forge: Forge, host: &str) -> Login {
        let token = stored_token(forge, host);
        let key = (forge, host.to_string(), token.as_deref().map_or(0, fingerprint));
        if let Some((login, at)) = self.known.get(&key) {
            if at.elapsed() < if login.ok { RECHECK_OK } else { RECHECK_LOGIN } {
                return *login;
            }
        }
        let out = run_tool(forge, host, &["auth", "status", "--hostname", host], LOGIN_TIMEOUT, token.as_deref());
        let ok = out.is_ok_and(|o| o.complete && o.success);
        let expires = token.as_deref().filter(|_| ok).and_then(|t| token_expiry(forge, host, t));
        let login = Login { ok, token: token.is_some(), expires };
        self.known.insert(key, (login, Instant::now()));
        login
    }

    /// What `path` says its version is (`glab --version`'s first line, without the date).
    fn version(&mut self, path: Option<&str>) -> Option<String> {
        let path = path?;
        self.versions
            .entry(path.to_string())
            .or_insert_with(|| {
                let out = crate::os::run(std::ffi::OsStr::new(path), &["--version"], LOGIN_TIMEOUT).ok()?;
                let text = out.stdout_text();
                let line = text.lines().next()?.trim();
                let line = line.split_once(" (").map_or(line, |(v, _)| v);
                (!line.is_empty()).then(|| line.to_string())
            })
            .clone()
    }

    /// How the two tools stand for the hosts in `hosts`, the hosts Trayward
    /// keeps a token for, and github.com for `gh` (Copilot's monthly limit is
    /// asked there) when it's installed or has a token.
    fn tools(&mut self, hosts: &[(Forge, String)]) -> Tools {
        let mut tools = Tools { asked: true, ..Tools::found() };
        let mut all: Vec<(Forge, String)> = hosts.to_vec();
        all.extend(token_hosts());
        if tools.gh.found {
            all.push((Forge::GitHub, GITHUB.to_string()));
        }
        all.sort_by(|a, b| (a.0 == Forge::GitHub, &a.1).cmp(&(b.0 == Forge::GitHub, &b.1)));
        all.dedup_by(|a, b| a.0 == b.0 && a.1.eq_ignore_ascii_case(&b.1));
        for (forge, host) in all {
            let login = self.check(forge, &host);
            let tool = match forge {
                Forge::GitLab => &mut tools.glab,
                Forge::GitHub => &mut tools.gh,
            };
            tool.hosts.push(Host { name: host, ok: login.ok, token: login.token, expires: login.expires });
        }
        tools.glab.version = self.version(tools.glab.path.as_deref());
        tools.gh.version = self.version(tools.gh.path.as_deref());
        tools
    }
}

/// GitHub's own host.
const GITHUB: &str = "github.com";

/// Asks every host the sessions' folders are on (of the GitLab hosts, only
/// those `glab` has a login for) for the sessions' branches and the user's own
/// requests in the sessions' projects.
fn fetch(folders: &[String], logins: &mut Logins) -> Snapshot {
    // Folder → repository and branch, grouped by host.
    // (forge, host) → (folder, project, branch)
    type Sessions = Vec<(String, String, String)>;
    let mut hosts: HashMap<(Forge, String), Sessions> = HashMap::new();
    for folder in folders {
        if let Some((repo, branch)) = repo_of(folder) {
            hosts.entry((repo.forge, repo.host)).or_default().push((folder.clone(), repo.path, branch));
        }
    }
    let asked_hosts: Vec<(Forge, String)> = hosts.keys().cloned().collect();
    let mut snap = Snapshot { tools: logins.tools(&asked_hosts), ..Default::default() };
    for ((forge, host), list) in hosts {
        if forge == Forge::GitLab && !logins.check(forge, &host).ok {
            continue;
        }
        // One question per project and branch, however many sessions share it.
        let mut asked: Vec<(String, String)> = Vec::new();
        for (_, path, branch) in &list {
            if !asked.iter().any(|(p, b)| p == path && b == branch) {
                asked.push((path.clone(), branch.clone()));
            }
        }
        let q = format!("query={}", query(forge, &asked));
        let answer = run_for(forge, &host, &["api", "graphql", "--hostname", &host, "-f", &q]);
        let Ok(answer) = answer.and_then(|a| serde_json::from_str::<Value>(&a).map_err(|e| e.to_string())) else {
            continue;
        };
        let (branches, mine) = parse(forge, &answer, asked.len());
        for (folder, path, branch) in &list {
            let i = asked.iter().position(|(p, b)| p == path && b == branch).unwrap_or(0);
            if let Some(Some((change, checks))) = branches.get(i) {
                snap.sessions
                    .insert(folder.clone(), Branch { name: branch.clone(), change: change.clone(), checks: *checks });
            }
        }
        // Only the user's requests in a project a session works in.
        let projects: Vec<&str> = list.iter().map(|(_, path, _)| path.as_str()).collect();
        snap.mine.extend(mine.into_iter().filter(|c| in_projects(c, &projects)));
    }
    sort_mine(&mut snap.mine);
    snap
}

/// Whether a request is in one of the projects (paths compared ignoring case:
/// a remote URL may spell the project differently from the host).
pub fn in_projects(c: &Change, projects: &[&str]) -> bool {
    projects.iter().any(|p| p.eq_ignore_ascii_case(&c.project))
}

/// The user's own requests across hosts: failed pipelines first (they need
/// the user), then running ones, then the most recently updated.
pub fn sort_mine(list: &mut [Change]) {
    let urgency = |c: &Change| match c.checks {
        Some(Checks::Failed) => 0,
        Some(Checks::Running) => 1,
        _ => 2,
    };
    list.sort_by_key(|c| (urgency(c), std::cmp::Reverse(c.updated)));
}

/// Asks the hosts every minute (and right away when `poke` is counted up) while
/// `on`, for the folders in `folders`; posts a [`Snapshot`] to `send` when
/// something changed.
pub fn spawn(
    send: impl Fn(Snapshot) + Send + 'static,
    on: Arc<AtomicBool>,
    poke: Arc<AtomicU64>,
    folders: Arc<Mutex<Vec<String>>>,
) {
    std::thread::spawn(move || {
        crate::os::background_thread();
        // glab's logins, asked for once it's switched on (not at every start).
        let mut logins = Logins::default();
        let mut last: Option<Snapshot> = None;
        let mut asked_at: Option<Instant> = None;
        let mut was_on = None;
        let mut seen = 0;
        loop {
            let is_on = on.load(Ordering::Relaxed);
            // Every minute, when asked to, and right when it's switched on or off.
            let pokes = poke.load(Ordering::Relaxed);
            let poked = std::mem::replace(&mut seen, pokes) != pokes;
            let due = asked_at.is_none_or(|t| t.elapsed() >= EVERY) || poked || was_on != Some(is_on);
            was_on = Some(is_on);
            if due {
                let list = folders.lock().map(|f| f.clone()).unwrap_or_default();
                let snap = match (is_on, crate::demo::active()) {
                    (true, true) => crate::demo::forge(&list),
                    (true, false) => fetch(&list, &mut logins),
                    // Off: nothing is run; only whether the tools are there is told.
                    (false, demo) => Snapshot {
                        tools: if demo { crate::demo::tools(false) } else { Tools::found() },
                        ..Default::default()
                    },
                };
                asked_at = Some(Instant::now());
                // Asked to (a token was pasted or removed): told even when nothing changed.
                if poked || last.as_ref() != Some(&snap) {
                    last = Some(snap.clone());
                    send(Snapshot { answers: pokes, ..snap });
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// What changed for the user between two snapshots: a pipeline finished, or a
/// merge or pull request was merged. Each change once, by its URL.
pub fn news(before: &Snapshot, now: &Snapshot) -> Vec<(Change, News)> {
    let all = |s: &Snapshot| -> HashMap<String, Change> {
        s.sessions
            .values()
            .filter_map(|b| b.change.clone())
            .chain(s.mine.iter().cloned())
            .map(|c| (c.url.clone(), c))
            .collect()
    };
    let (old, new) = (all(before), all(now));
    let mut out: Vec<(Change, News)> = Vec::new();
    for (url, c) in new {
        let Some(o) = old.get(&url) else { continue };
        let what = if o.state == State::Open && c.state == State::Merged {
            Some(News::Merged)
        } else if o.checks == Some(Checks::Running) {
            match c.checks {
                Some(Checks::Passed) => Some(News::Passed),
                Some(Checks::Failed) => Some(News::Failed),
                Some(Checks::Canceled) => Some(News::Canceled),
                _ => None,
            }
        } else {
            None
        };
        if let Some(w) = what {
            out.push((c, w));
        }
    }
    // Failures first: they need the user.
    out.sort_by_key(|(_, w)| *w != News::Failed);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum News {
    Passed,
    Failed,
    Canceled,
    Merged,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_its_tokens_in_the_credential_store() {
        assert_eq!(token_target(Forge::GitLab, "GitLab.Example.com"), "trayward:gitlab:gitlab.example.com");
        assert_eq!(token_target(Forge::GitHub, "github.com"), "trayward:github:github.com");
        assert_eq!(
            token_host("trayward:gitlab:gitlab.example.com"),
            Some((Forge::GitLab, "gitlab.example.com".into()))
        );
        assert_eq!(token_host("trayward:other:x"), None);
        assert_eq!(token_host("trayward:github:"), None);
        assert_eq!(token_host("git:https://github.com"), None);
    }

    #[test]
    fn hands_a_token_over_in_the_environment() {
        let env = token_env(Forge::GitLab, "gitlab.example.com", "glpat-fake");
        assert_eq!(env, [("GITLAB_TOKEN", "glpat-fake".into()), ("GITLAB_HOST", "gitlab.example.com".into())]);
        assert_eq!(token_env(Forge::GitHub, "github.com", "ghp_fake"), [("GH_TOKEN", "ghp_fake".into())]);
        let enterprise = token_env(Forge::GitHub, "git.example.com", "ghp_fake");
        assert_eq!(enterprise, [("GH_ENTERPRISE_TOKEN", "ghp_fake".into()), ("GH_HOST", "git.example.com".into())]);
    }

    #[test]
    fn takes_only_a_token_from_the_clipboard() {
        // Made-up tokens, put together here so no secret scanner takes them for real ones.
        assert!(looks_like_token(&format!("glpat-{}", "Fake".repeat(5))));
        assert!(looks_like_token(&format!("github_pat_{}", "Example0".repeat(5))));
        assert!(!looks_like_token("too-short"));
        assert!(!looks_like_token("a sentence that was on the clipboard"));
        assert!(!looks_like_token("https://gitlab.example.com/-/user_settings"));
        assert!(!looks_like_token(r"C:\Users\someone\token.txt"));
    }

    #[test]
    fn never_shows_a_token() {
        let said = "401 for glpat-AbCdEf_123-x: bad token ghp_fake123 (and secret-of-mine)";
        assert_eq!(redact(said, Some("secret-of-mine")), "401 for [token]: bad token [token] (and [token])");
        assert_eq!(redact("nothing here", None), "nothing here");
    }

    #[test]
    fn reads_when_a_token_expires() {
        let day = |s: &str| crate::timefmt::parse_rfc3339(s).unwrap();
        assert_eq!(date_at_start("2027-11-21"), Some(day("2027-11-21T00:00:00Z")));
        let answer = "HTTP/2.0 200 OK\nContent-Type: application/json\nGithub-Authentication-Token-Expiration: 2027-11-21 12:00:00 +0100\n\n{\"login\":\"someone\"}";
        assert_eq!(github_expiry(answer), Some(day("2027-11-21T00:00:00Z")));
        // Only in the headers, and none for a token that doesn't expire.
        assert_eq!(github_expiry("HTTP/2.0 200 OK\n\ngithub-authentication-token-expiration: 2027-11-21"), None);
        assert_eq!(date_at_start("never"), None);
    }
    use serde_json::json;

    #[test]
    fn reads_remotes() {
        let r = parse_remote("git@gitlab.example.com:group/sub/project.git").unwrap();
        assert_eq!(
            (r.forge, r.host.as_str(), r.path.as_str()),
            (Forge::GitLab, "gitlab.example.com", "group/sub/project")
        );
        let r = parse_remote("https://github.com/owner/repo.git").unwrap();
        assert_eq!((r.forge, r.path.as_str()), (Forge::GitHub, "owner/repo"));
        let r = parse_remote("ssh://git@gitlab.example.com:2222/group/project").unwrap();
        assert_eq!((r.host.as_str(), r.path.as_str()), ("gitlab.example.com", "group/project"));
        let r = parse_remote("https://user@GitLab.Example.com/group/project/").unwrap();
        assert_eq!(r.host, "gitlab.example.com");
        assert!(parse_remote("/local/path").is_none());
        // Hosts that aren't GitLab are left alone.
        assert!(parse_remote("git@bitbucket.org:team/repo.git").is_none());
        assert!(parse_remote("https://dev.azure.com/org/project/_git/repo").is_none());
        assert!(parse_remote("https://org.visualstudio.com/project/_git/repo").is_none());
        assert!(parse_remote("https://git.code.sf.net/p/proj/code").is_none());
        assert!(parse_remote("https://hf.co/org/model").is_none());
        let config =
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = x\n[remote \"origin\"]\n\turl = git@h:g/p.git\n";
        assert_eq!(origin_url(config).as_deref(), Some("git@h:g/p.git"));
    }

    #[test]
    fn finds_the_branch_also_in_a_worktree() {
        let base = std::env::temp_dir().join(format!("trayward-forge-test-{}", std::process::id()));
        let main = base.join("repo");
        let git = main.join(".git");
        std::fs::create_dir_all(git.join("worktrees").join("wt")).unwrap();
        std::fs::write(git.join("config"), "[remote \"origin\"]\n\turl = git@gitlab.example.com:g/p.git\n").unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(main.join("src")).unwrap();
        let (repo, branch) = repo_of(&main.join("src").display().to_string()).unwrap();
        assert_eq!((repo.path.as_str(), branch.as_str()), ("g/p", "main"));
        // A worktree: `.git` is a file; its HEAD is its own, the config shared.
        let wt = base.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let wt_git = git.join("worktrees").join("wt");
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        std::fs::write(wt_git.join("HEAD"), "ref: refs/heads/fix/api-base-url\n").unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let (repo, branch) = repo_of(&wt.display().to_string()).unwrap();
        assert_eq!((repo.path.as_str(), branch.as_str()), ("g/p", "fix/api-base-url"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn asks_once_per_host() {
        let q = query(Forge::GitLab, &[("g/p".into(), "main".into()), ("g/q".into(), "fix \"x\"".into())]);
        assert!(q.contains(r#"b0: project(fullPath: "g/p")"#) && q.contains(r#"sourceBranches: ["fix \"x\""]"#));
        assert!(q.contains("mine: currentUser"));
        let q = query(Forge::GitHub, &[("o/r".into(), "feat".into())]);
        assert!(q.contains(r#"repository(owner: "o", name: "r")"#) && q.contains("mine: viewer"));
    }

    #[test]
    fn reads_the_answers() {
        let gitlab = json!({"data": {
            "b0": {"mergeRequests": {"nodes": [{"iid": "37", "title": "Fix login", "webUrl": "https://h/g/p/-/merge_requests/37",
                    "state": "opened", "draft": false, "project": {"fullPath": "g/p"}, "headPipeline": {"status": "RUNNING"}}]},
                   "pipelines": {"nodes": [{"status": "SUCCESS"}]}},
            "b1": {"mergeRequests": {"nodes": []}, "pipelines": {"nodes": [{"status": "FAILED"}]}},
            "b2": null,
            "mine": {"authoredMergeRequests": {"nodes": [{"iid": "5", "title": "Docs", "webUrl": "https://h/g/q/-/merge_requests/5",
                    "state": "opened", "draft": true, "project": {"fullPath": "g/q"}, "headPipeline": null}]}}
        }});
        let (branches, mine) = parse(Forge::GitLab, &gitlab, 3);
        let (change, checks) = branches[0].clone().unwrap();
        let change = change.unwrap();
        assert_eq!(
            (change.reference(), change.checks, checks),
            ("!37".into(), Some(Checks::Running), Some(Checks::Running))
        );
        // No merge request: the branch's pipeline.
        assert_eq!(branches[1], Some((None, Some(Checks::Failed))));
        assert_eq!(branches[2], None);
        assert!(mine[0].draft && mine[0].checks.is_none() && mine[0].project == "g/q");

        let github = json!({"data": {
            "b0": {"pullRequests": {"nodes": [{"number": 12, "title": "Feat", "url": "https://github.com/o/r/pull/12", "state": "MERGED",
                   "isDraft": false, "repository": {"nameWithOwner": "o/r"},
                   "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS"}}}]}}]}},
            "mine": {"pullRequests": {"nodes": []}}
        }});
        let (branches, mine) = parse(Forge::GitHub, &github, 1);
        let change = branches[0].clone().unwrap().0.unwrap();
        assert_eq!(
            (change.reference(), change.state, change.checks),
            ("#12".into(), State::Merged, Some(Checks::Passed))
        );
        assert!(mine.is_empty());
    }

    #[test]
    fn keeps_only_the_sessions_projects() {
        let c = |project: &str| Change {
            forge: Forge::GitLab,
            number: 1,
            title: String::new(),
            url: String::new(),
            project: project.into(),
            state: State::Open,
            draft: false,
            checks: None,
            updated: 0,
        };
        let projects = ["Group/Shop", "group/api"];
        assert!(in_projects(&c("group/shop"), &projects));
        assert!(in_projects(&c("group/api"), &projects));
        assert!(!in_projects(&c("group/api-docs"), &projects));
        assert!(!in_projects(&c("other/thing"), &projects));
    }

    #[test]
    fn lists_what_needs_the_user_first() {
        let c = |number, checks, updated| Change {
            forge: Forge::GitHub,
            number,
            title: String::new(),
            url: String::new(),
            project: String::new(),
            state: State::Open,
            draft: false,
            checks,
            updated,
        };
        let mut list = vec![
            c(1, None, 50),
            c(2, Some(Checks::Passed), 90),
            c(3, Some(Checks::Failed), 10),
            c(4, Some(Checks::Running), 5),
        ];
        sort_mine(&mut list);
        assert_eq!(list.iter().map(|c| c.number).collect::<Vec<_>>(), [3, 4, 2, 1]);
    }

    #[test]
    fn tells_what_happened() {
        let mr = |checks, state| Change {
            forge: Forge::GitLab,
            number: 1,
            title: "T".into(),
            url: "u".into(),
            project: "g/p".into(),
            state,
            draft: false,
            checks,
            updated: 0,
        };
        let snap = |c: Change| Snapshot { mine: vec![c], ..Snapshot::default() };
        let running = snap(mr(Some(Checks::Running), State::Open));
        assert_eq!(news(&running, &snap(mr(Some(Checks::Failed), State::Open)))[0].1, News::Failed);
        assert_eq!(news(&running, &snap(mr(Some(Checks::Passed), State::Open)))[0].1, News::Passed);
        assert_eq!(news(&snap(mr(None, State::Open)), &snap(mr(None, State::Merged)))[0].1, News::Merged);
        // Nothing new, or seen for the first time: nothing to say.
        assert!(news(&running, &running).is_empty());
        assert!(news(&Snapshot::default(), &running).is_empty());
        // A session's branch counts too; a canceled pipeline is news, a pipeline that runs on isn't.
        let branch = |c: Change| Snapshot {
            sessions: HashMap::from([(
                "C:\\work\\p".to_string(),
                Branch { name: "fix".into(), checks: c.checks, change: Some(c) },
            )]),
            ..Snapshot::default()
        };
        let on_branch = branch(mr(Some(Checks::Running), State::Open));
        assert_eq!(news(&on_branch, &branch(mr(Some(Checks::Canceled), State::Open)))[0].1, News::Canceled);
        assert!(news(&on_branch, &branch(mr(None, State::Open))).is_empty());
        assert!(
            news(&snap(mr(Some(Checks::Passed), State::Open)), &snap(mr(Some(Checks::Failed), State::Open))).is_empty()
        );
        // Failures first.
        let two = |a: Change, b: Change| Snapshot { mine: vec![a, b], ..Snapshot::default() };
        let other = |c: Change| Change { url: "v".into(), ..c };
        let before = two(mr(Some(Checks::Running), State::Open), other(mr(Some(Checks::Running), State::Open)));
        let after = two(mr(Some(Checks::Passed), State::Open), other(mr(Some(Checks::Failed), State::Open)));
        assert_eq!(news(&before, &after).iter().map(|(_, n)| *n).collect::<Vec<_>>(), [News::Failed, News::Passed]);
    }

    #[test]
    fn reads_every_pipeline_state() {
        for (status, checks) in [
            ("running", Some(Checks::Running)),
            ("WAITING_FOR_RESOURCE", Some(Checks::Running)),
            ("EXPECTED", Some(Checks::Running)),
            ("SUCCESS", Some(Checks::Passed)),
            ("FAILURE", Some(Checks::Failed)),
            ("ERROR", Some(Checks::Failed)),
            ("CANCELED", Some(Checks::Canceled)),
            ("canceling", Some(Checks::Canceled)),
            ("SKIPPED", None),
            ("MANUAL", None),
        ] {
            assert_eq!(checks_of(status), checks, "{status}");
        }
    }

    #[test]
    fn skips_what_it_does_not_understand_in_an_answer() {
        let gitlab = |v: Value| change_of(Forge::GitLab, &v);
        // A number that isn't one, no address, no state: nothing.
        assert_eq!(gitlab(json!({"iid": "x", "webUrl": "u", "state": "opened"})), None);
        assert_eq!(gitlab(json!({"iid": "1", "state": "opened"})), None);
        assert_eq!(gitlab(json!({"iid": "1", "webUrl": "u"})), None);
        assert_eq!(change_of(Forge::GitHub, &json!({"number": "1", "url": "u", "state": "OPEN"})), None);
        // The least there can be: no title, project, pipeline or time.
        let c = gitlab(json!({"iid": "3", "webUrl": "u", "state": "closed"})).unwrap();
        assert_eq!(
            (c.state, c.title.as_str(), c.project.as_str(), c.checks, c.updated),
            (State::Closed, "", "", None, 0)
        );
        let c = change_of(
            Forge::GitHub,
            &json!({"number": 4, "url": "u", "state": "OPEN", "isDraft": true,
            "updatedAt": "2026-11-12T10:00:00Z", "commits": {"nodes": []}}),
        )
        .unwrap();
        assert!(c.draft && c.state == State::Open && c.checks.is_none() && c.updated > 0);
        // An answer without data (an error) or without the user's own.
        let (branches, mine) = parse(Forge::GitHub, &json!({"errors": [{"message": "x"}]}), 2);
        assert_eq!((branches, mine), (vec![None, None], Vec::new()));
        // A GitHub branch without a pull request: no pipeline of its own.
        let answer = json!({"data": {"b0": {"pullRequests": {"nodes": []}}}});
        assert_eq!(parse(Forge::GitHub, &answer, 1).0, vec![Some((None, None))]);
    }

    #[test]
    fn reads_odd_remotes_and_configs() {
        assert!(parse_remote("https:///group/project").is_none());
        assert!(parse_remote("git@gitlab.example.com:project").is_none());
        assert!(parse_remote("https://gitlab.example.com").is_none());
        let r = parse_remote("ssh://git@GitHub.com/o/r.git").unwrap();
        assert_eq!((r.forge, r.host.as_str()), (Forge::GitHub, "github.com"));
        assert_eq!(origin_url("[remote \"origin\"]\n\tfetch = +refs/heads/*\n\tnot a setting\n"), None);
        assert_eq!(origin_url("[remote \"origin\"]\n[core]\n\turl = elsewhere\n"), None);
        assert_eq!(origin_url(""), None);
        assert_eq!(program(Forge::GitLab), "glab");
        assert_eq!(program(Forge::GitHub), "gh");
        assert_eq!(date_at_start("2027-11"), None);
        assert_eq!(github_expiry(""), None);
    }

    #[test]
    fn a_folder_without_a_branch_has_none() {
        let base = std::env::temp_dir().join(format!("trayward-forge-odd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Detached HEAD: no branch to ask about.
        let detached = base.join("detached");
        std::fs::create_dir_all(detached.join(".git")).unwrap();
        std::fs::write(detached.join(".git").join("HEAD"), "0123456789abcdef\n").unwrap();
        assert!(repo_of(&detached.display().to_string()).is_none());
        // No config, or no origin in it.
        let bare = base.join("bare");
        std::fs::create_dir_all(bare.join(".git")).unwrap();
        std::fs::write(bare.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert!(repo_of(&bare.display().to_string()).is_none());
        std::fs::write(bare.join(".git").join("config"), "[core]\n").unwrap();
        assert!(repo_of(&bare.display().to_string()).is_none());
        // A `.git` file that isn't a pointer; a worktree without a common folder of its own.
        let odd = base.join("odd");
        std::fs::create_dir_all(&odd).unwrap();
        std::fs::write(odd.join(".git"), "something else\n").unwrap();
        assert!(git_dirs(&odd).is_none());
        let wt = base.join("wt");
        let own = base.join("wt-git");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", own.display())).unwrap();
        assert_eq!(git_dirs(&wt), Some((own.clone(), own.clone())));
        std::fs::write(own.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
        std::fs::write(own.join("config"), "[remote \"origin\"]\n\turl = https://github.com/o/r\n").unwrap();
        let (repo, branch) = repo_of(&wt.display().to_string()).unwrap();
        assert_eq!((repo.forge, repo.path.as_str(), branch.as_str()), (Forge::GitHub, "o/r", "feat"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn asks_a_tool_its_version_once() {
        let mut logins = Logins::default();
        assert_eq!(logins.version(None), None);
        assert_eq!(logins.version(Some("C:\\no-such-folder\\no-such-tool.exe")), None);
        // Cargo answers like glab: "cargo 1.90.0 (hash date)" → without the part in brackets.
        let cargo = env!("CARGO");
        let version = logins.version(Some(cargo)).unwrap();
        assert!(version.starts_with("cargo ") && !version.contains('('), "{version}");
        // The missing one is remembered too: not tried again every minute.
        assert_eq!(logins.versions.len(), 2);
        assert_eq!(logins.version(Some(cargo)), Some(version));
    }

    #[test]
    fn tells_whether_the_tools_are_there_while_off() {
        // Looked up in PATH only: nothing is run while it's off.
        let found = Tools::found();
        assert!(!found.asked && found.glab.hosts.is_empty() && found.gh.version.is_none());
        assert_eq!(found.glab.found, found.glab.path.is_some());
        let (tx, rx) = std::sync::mpsc::channel();
        let poke = Arc::new(AtomicU64::new(0));
        let folders = Arc::new(Mutex::new(vec!["C:\\work\\p".to_string()]));
        spawn(move |s| drop(tx.send(s)), Arc::new(AtomicBool::new(false)), poke.clone(), folders);
        let first = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!((first.answers, first.tools.clone(), first.sessions.len()), (0, found.clone(), 0));
        // Poked (a token was pasted): told again, though nothing changed.
        poke.store(1, Ordering::Relaxed);
        let again = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!((again.answers, again.tools), (1, found));
    }

    #[test]
    fn never_shows_a_token_anywhere_in_a_message() {
        assert_eq!(redact("ends with gldt-abc", None), "ends with [token]");
        assert_eq!(redact("glptt-x and gho_y", Some("")), "[token] and [token]");
        assert_eq!(redact("", Some("secret")), "");
        assert!(looks_like_token(&"a".repeat(255)));
        assert!(!looks_like_token(&"a".repeat(256)));
        assert!(!looks_like_token(&format!("{}\u{e9}", "a".repeat(30))));
    }
}
