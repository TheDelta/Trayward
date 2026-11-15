//! Background thread that gathers everything about sessions that means
//! reading files (some of them over the slow WSL share): the session lists of
//! Windows and of running WSL distributions, transcripts, and the names of
//! VS Code windows. The UI thread asks for a pass and gets a [`Snapshot`].

use crate::registry::{self, Source};
use crate::sessions::Listed;
use crate::transcripts::{Scanner, Stats};
use crate::vscode;
use crate::win::{SendHwnd, post_boxed};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

/// A session the app already knows (e.g. from hooks), so its transcript is
/// read even when it isn't in a session list.
#[derive(Clone, Debug)]
pub struct Known {
    pub id: String,
    pub cwd: String,
    pub distro: Option<String>,
}

/// Result of one pass.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// Running sessions from all session lists.
    pub listed: Vec<Listed>,
    /// When the session lists were read (unix ms): what they say is as of then.
    pub listed_at_ms: i64,
    /// WSL distributions that are running.
    pub wsl_running: Vec<String>,
    /// Running distributions whose session lists were read successfully.
    pub wsl_scanned: Vec<String>,
    /// Transcript stats that changed.
    pub stats: Vec<(String, Stats)>,
    /// VS Code window names that changed.
    pub workspaces: Vec<(String, Option<String>)>,
    /// VS Code's exe, when it was looked up in this pass.
    pub vscode_exe: Option<PathBuf>,
    /// Sessions archived in the Claude Code extension, when read in this pass.
    pub archived: Option<std::collections::HashSet<String>>,
    /// The GPG signing key's cache, when checked in this pass.
    pub gpg: Option<GpgReading>,
}

/// What the collector saw of the GPG signing key.
#[derive(Clone, Debug, PartialEq)]
pub struct GpgReading {
    pub cached: bool,
    /// The agent's process id, while one runs.
    pub agent: Option<u32>,
    /// When the agent started (unix seconds): the key can't have been unlocked before.
    pub agent_started: Option<i64>,
    pub default_ttl: u64,
    pub max_ttl: u64,
    /// Latest signed commit in the repositories of the known sessions (unix seconds).
    pub last_commit: Option<i64>,
}

/// How often the GPG agent is asked (it starts a small program).
const GPG_EVERY: Duration = Duration::from_secs(30);
/// How often the signing setup (git config, keys) is looked up again.
const GPG_SETUP_EVERY: Duration = Duration::from_secs(10 * 60);

/// How often running WSL distributions are looked up (it starts `wsl.exe`).
const WSL_EVERY: Duration = Duration::from_secs(15);
/// How long a transcript that wasn't found in any config folder isn't looked for.
const LOOK_AGAIN: Duration = Duration::from_secs(30);
/// How long a window name is reused before reading settings again.
const NAME_TTL: Duration = Duration::from_secs(30);

struct Collector {
    config_dir: PathBuf,
    scanner: Scanner,
    wsl_checked: Option<Instant>,
    wsl_sources: Vec<Source>,
    wsl_running: Vec<String>,
    names: HashMap<(Option<String>, String), (Instant, Option<String>)>,
    /// Which config folder a session's transcript is in, where there are
    /// several (none: not found when last looked).
    session_configs: HashMap<String, (Option<PathBuf>, Instant)>,
    reported_names: HashMap<String, Option<String>>,
    gpg_setup: Option<(Instant, Option<crate::gpg::Setup>)>,
    gpg_checked: Option<Instant>,
}

impl Collector {
    /// Claude Code's config folders for Windows or a distribution. A
    /// distribution can have two: root's and a user's.
    fn configs_for(&self, distro: Option<&str>) -> Vec<PathBuf> {
        match distro {
            None => vec![self.config_dir.clone()],
            Some(d) => self
                .wsl_sources
                .iter()
                .filter(|s| s.distro.as_deref() == Some(d))
                .map(|s| s.config_dir.clone())
                .collect(),
        }
    }

    /// The config folder a session's transcript is in: of several, the one
    /// that has it (remembered once found), else the first.
    fn config_of(&mut self, id: &str, cwd: &str, distro: Option<&str>) -> Option<PathBuf> {
        let configs = self.configs_for(distro);
        if configs.len() > 1 {
            match self.session_configs.get(id) {
                Some((Some(known), _)) => return Some(known.clone()),
                // Looked for recently and not found (a new session): not again yet.
                Some((None, at)) if at.elapsed() < LOOK_AGAIN => return configs.into_iter().next(),
                _ => {}
            }
            let found = configs.iter().find(|c| crate::transcripts::find(c, id, cwd).is_some()).cloned();
            self.session_configs.insert(id.to_string(), (found.clone(), Instant::now()));
            if found.is_some() {
                return found;
            }
        }
        configs.into_iter().next()
    }

    /// Name of the VS Code window holding `cwd`.
    fn window_name(&mut self, distro: Option<&str>, cwd: &str, user_template: Option<&str>) -> Option<String> {
        let key = (distro.map(str::to_string), cwd.to_string());
        if let Some((at, name)) = self.names.get(&key) {
            if at.elapsed() < NAME_TTL {
                return name.clone();
            }
        }
        let locks: Vec<vscode::IdeLock> =
            self.configs_for(distro).iter().flat_map(|c| vscode::read_locks(c, distro.is_none())).collect();
        let folder = match distro {
            None => vscode::windows_folder(&locks, cwd, user_template),
            Some(_) => vscode::workspace_folder(&locks, cwd),
        };
        let name = folder.map(|f| {
            let path = match distro {
                None => PathBuf::from(&f),
                Some(d) => registry::wsl_path(d, &f),
            };
            vscode::window_name(&path, user_template)
        });
        self.names.insert(key, (Instant::now(), name.clone()));
        name
    }

    /// Checks the signing key's cache, now and then.
    fn gpg(&mut self, force: bool, known: &[Known]) -> Option<GpgReading> {
        if !force && self.gpg_checked.is_some_and(|t| t.elapsed() < GPG_EVERY) {
            return None;
        }
        self.gpg_checked = Some(Instant::now());
        if force || self.gpg_setup.as_ref().is_none_or(|(t, _)| t.elapsed() >= GPG_SETUP_EVERY) {
            let found = crate::gpg::detect();
            // Still signing but the key couldn't be looked up just now: keep the last setup.
            let previous = self.gpg_setup.take().and_then(|(_, s)| s);
            let setup = found.or_else(|| previous.filter(|_| crate::gpg::signs()));
            self.gpg_setup = Some((Instant::now(), setup));
        }
        let setup = self.gpg_setup.as_ref()?.1.as_ref()?;
        let agent = crate::gpg::agent(setup)?;
        // Commits in Windows repositories are signed by this agent; WSL has its own.
        let last_commit = known
            .iter()
            .filter(|k| k.distro.is_none() && !k.cwd.is_empty())
            .filter_map(|k| crate::gpg::last_commit(Path::new(&k.cwd)))
            .max();
        Some(GpgReading {
            cached: agent.cached,
            agent: agent.pid,
            // FILETIME ticks (100 ns since 1601) to unix seconds.
            agent_started: agent
                .pid
                .and_then(crate::win::process_start_time)
                .map(|t| (t / 10_000_000) as i64 - 11_644_473_600),
            default_ttl: setup.default_ttl,
            max_ttl: setup.max_ttl,
            last_commit,
        })
    }

    fn pass(&mut self, req: &Request) -> Snapshot {
        let known = &req.known;
        let mut vscode_exe = None;
        let mut archived = None;
        if req.force || self.wsl_checked.is_none_or(|t| t.elapsed() >= WSL_EVERY) {
            vscode_exe = vscode::running_exe();
            archived = crate::vscode_state::archived_sessions();
            if req.wsl {
                // Couldn't be asked (wsl.exe slow or failing): the last answer stays,
                // rather than every WSL session being dropped.
                if let Some(running) = registry::wsl_running() {
                    self.wsl_running = running;
                    self.wsl_sources = self.wsl_running.iter().flat_map(|d| registry::wsl_sources(d)).collect();
                }
            }
            self.wsl_checked = Some(Instant::now());
        }
        if !req.wsl {
            self.wsl_running.clear();
            self.wsl_sources.clear();
        }
        let windows = Source { distro: None, config_dir: self.config_dir.clone() };
        let listed_at_ms = crate::win::now_ms();
        let mut listed = registry::list(&windows).unwrap_or_default();
        let mut scanned: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for source in &self.wsl_sources {
            let distro = source.distro.clone().unwrap_or_default();
            match registry::list(source) {
                Some(l) => listed.extend(l),
                None => failed.push(distro.clone()),
            }
            if !scanned.contains(&distro) {
                scanned.push(distro);
            }
        }
        scanned.retain(|d| !failed.contains(d));

        // Transcripts of listed sessions and of other sessions the app knows.
        let mut targets: Vec<(String, String, Option<String>)> =
            listed.iter().map(|l| (l.id.clone(), l.cwd.clone(), l.distro.clone())).collect();
        for k in known {
            if !targets.iter().any(|(id, _, _)| id == &k.id) {
                targets.push((k.id.clone(), k.cwd.clone(), k.distro.clone()));
            }
        }
        self.session_configs.retain(|id, _| targets.iter().any(|(t, _, _)| t == id));
        let mut with_config: Vec<(String, String, PathBuf)> = Vec::new();
        for (id, cwd, d) in &targets {
            if let Some(config) = self.config_of(id, cwd, d.as_deref()) {
                with_config.push((id.clone(), cwd.clone(), config));
            }
        }
        let stats = if req.transcripts { self.scanner.scan(&with_config) } else { Vec::new() };

        let user_template = vscode::user_window_title();
        let mut workspaces = Vec::new();
        for (id, cwd, distro) in &targets {
            let name = self.window_name(distro.as_deref(), cwd, user_template.as_deref());
            if self.reported_names.get(id) != Some(&name) {
                self.reported_names.insert(id.clone(), name.clone());
                workspaces.push((id.clone(), name));
            }
        }
        self.reported_names.retain(|id, _| targets.iter().any(|(t, _, _)| t == id));

        let gpg = if req.gpg { self.gpg(req.force, known) } else { None };
        Snapshot {
            listed,
            listed_at_ms,
            wsl_running: self.wsl_running.clone(),
            wsl_scanned: scanned,
            stats,
            workspaces,
            vscode_exe,
            archived,
            gpg,
        }
    }
}

/// A request to the collector: the sessions the app knows, and whether to
/// look for WSL distributions right away (manual refresh).
pub struct Request {
    pub known: Vec<Known>,
    pub force: bool,
    /// Read transcripts (titles, tokens, cache timers).
    pub transcripts: bool,
    /// Look for sessions in running WSL distributions.
    pub wsl: bool,
    /// Check the GPG signing key's cache.
    pub gpg: bool,
}

/// Starts the collector. Each request triggers a pass whose [`Snapshot`] is
/// posted to `ctl` as message `msg`.
pub fn spawn(config_dir: &Path, ctl: SendHwnd, msg: u32) -> Sender<Request> {
    let (tx, rx) = channel::<Request>();
    let mut c = Collector {
        config_dir: config_dir.to_path_buf(),
        scanner: Scanner::default(),
        wsl_checked: None,
        wsl_sources: Vec::new(),
        wsl_running: Vec::new(),
        names: HashMap::new(),
        session_configs: HashMap::new(),
        reported_names: HashMap::new(),
        gpg_setup: None,
        gpg_checked: None,
    };
    std::thread::spawn(move || {
        while let Ok(mut req) = rx.recv() {
            // Only the newest request matters, but a forced one stays forced.
            while let Ok(newer) = rx.try_recv() {
                let force = req.force || newer.force;
                req = Request { force, ..newer };
            }
            let snapshot = c.pass(&req);
            post_boxed(ctl, msg, snapshot);
        }
    });
    tx
}
