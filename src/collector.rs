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
    /// How the GPG key timer's setup stands, when looked at in this pass.
    pub gpg_check: Option<crate::gpg::Check>,
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
/// How often a distribution without sessions is looked into. WSL stops a
/// distribution about 15 seconds after its last process ended, and looking into
/// its files counts as use: read every pass, an unused one would never stop.
const WSL_QUIET_EVERY: Duration = Duration::from_secs(30);
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
    /// When each running distribution was last looked into, and whether it had sessions then.
    wsl_looked: HashMap<String, (Instant, bool)>,
    /// The distributions this pass may read: asked a moment ago and running.
    wsl_now: Vec<String>,
    names: HashMap<(Option<String>, String), (Instant, Option<String>)>,
    /// Which config folder a session's transcript is in, where there are
    /// several (none: not found when last looked).
    session_configs: HashMap<String, (Option<PathBuf>, Instant)>,
    reported_names: HashMap<String, Option<String>>,
    gpg_setup: Option<(Instant, Result<crate::gpg::Setup, crate::gpg::Missing>)>,
    gpg_checked: Option<Instant>,
    /// While the GPG key timer is off: when git and GnuPG were last looked up.
    gpg_found: Option<Instant>,
}

impl Collector {
    fn new(config_dir: &Path) -> Self {
        Collector {
            config_dir: config_dir.to_path_buf(),
            scanner: Scanner::default(),
            wsl_checked: None,
            wsl_sources: Vec::new(),
            wsl_running: Vec::new(),
            wsl_looked: HashMap::new(),
            wsl_now: Vec::new(),
            names: HashMap::new(),
            session_configs: HashMap::new(),
            reported_names: HashMap::new(),
            gpg_setup: None,
            gpg_checked: None,
            gpg_found: None,
        }
    }

    /// Claude Code's config folders for Windows or a distribution. A
    /// distribution can have two: root's and a user's. None of a distribution
    /// this pass doesn't read.
    fn configs_for(&self, distro: Option<&str>) -> Vec<PathBuf> {
        match distro {
            None => vec![self.config_dir.clone()],
            Some(d) if !self.wsl_now.iter().any(|n| n == d) => Vec::new(),
            Some(d) => self
                .wsl_sources
                .iter()
                .filter(|s| s.distro.as_deref() == Some(d))
                .map(|s| s.config_dir.clone())
                .collect(),
        }
    }

    /// Whether a running distribution is looked into in this pass.
    fn looks_into(&self, distro: &str, known: &[Known]) -> bool {
        let last = self.wsl_looked.get(distro).map(|(at, sessions)| (at.elapsed(), *sessions));
        look_now(last, known.iter().any(|k| k.distro.as_deref() == Some(distro)))
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

    /// The GPG key timer's reading and how its setup stands, when looked at in
    /// this pass. Switched off, nothing is run: git and GnuPG are only looked up,
    /// now and then.
    fn gpg_pass(&mut self, req: &Request, known: &[Known]) -> (Option<GpgReading>, Option<crate::gpg::Check>) {
        if !req.gpg {
            self.gpg_checked = None;
            if !req.force && self.gpg_found.is_some_and(|t| t.elapsed() < GPG_SETUP_EVERY) {
                return (None, None);
            }
            self.gpg_found = Some(Instant::now());
            let check = if crate::demo::active() { crate::demo::gpg_check(false) } else { crate::gpg::found() };
            return (None, Some(check));
        }
        self.gpg_found = None;
        // (Not in the demo: it would run the user's git and gpg.)
        if crate::demo::active() {
            return (None, Some(crate::demo::gpg_check(true)));
        }
        match self.gpg(req.force, known) {
            Some((reading, check)) => (reading, Some(check)),
            None => (None, None),
        }
    }

    /// Checks the signing key's cache, now and then: the reading, if there's a
    /// key to watch, and how the setup stands.
    fn gpg(&mut self, force: bool, known: &[Known]) -> Option<(Option<GpgReading>, crate::gpg::Check)> {
        use crate::gpg::{Check, Missing};
        if !force && self.gpg_checked.is_some_and(|t| t.elapsed() < GPG_EVERY) {
            return None;
        }
        self.gpg_checked = Some(Instant::now());
        if force || self.gpg_setup.as_ref().is_none_or(|(t, _)| t.elapsed() >= GPG_SETUP_EVERY) {
            let found = crate::gpg::detect();
            // Still signing but the key couldn't be looked up just now: keep the last setup.
            let previous = self.gpg_setup.take().and_then(|(_, s)| s.ok());
            let setup = found.or_else(|why| previous.filter(|_| crate::gpg::signs()).ok_or(why));
            self.gpg_setup = Some((Instant::now(), setup));
        }
        let setup = match &self.gpg_setup.as_ref()?.1 {
            Ok(setup) => setup,
            Err(why) => return Some((None, Check::Missing(*why))),
        };
        let Some(agent) = crate::gpg::agent(setup) else {
            return Some((None, Check::Missing(Missing::Agent)));
        };
        let check = Check::Ready { running: agent.pid.is_some(), cached: agent.cached };
        // Commits in Windows repositories are signed by this agent; WSL has its own.
        let last_commit = known
            .iter()
            .filter(|k| k.distro.is_none() && !k.cwd.is_empty())
            .filter_map(|k| crate::gpg::last_commit(Path::new(&k.cwd)))
            .max();
        let reading = GpgReading {
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
        };
        Some((Some(reading), check))
    }

    fn pass(&mut self, req: &Request) -> Snapshot {
        let mut vscode_exe = None;
        let mut archived = None;
        let due = req.force || self.wsl_checked.is_none_or(|t| t.elapsed() >= WSL_EVERY);
        if due {
            vscode_exe = vscode::running_exe();
            archived = crate::vscode_state::archived_sessions();
            self.wsl_checked = Some(Instant::now());
        }
        // The demo reads nothing of the user's, whatever is switched on in it.
        let wsl = req.wsl && !crate::demo::active();
        // Which distributions run is asked now and then, to find new ones, and before
        // every pass that would read one: reading a stopped one would start it again.
        self.wsl_now.clear();
        if wsl && (due || self.wsl_running.iter().any(|d| self.looks_into(d, &req.known))) {
            // Couldn't be asked (wsl.exe slow or failing): the last answer stays,
            // rather than every WSL session being dropped, but nothing is read
            // from a distribution that may have stopped since.
            if let Some(running) = registry::wsl_running() {
                self.wsl_looked.retain(|d, _| running.contains(d));
                self.wsl_sources.retain(|s| s.distro.as_ref().is_some_and(|d| running.contains(d)));
                for d in &running {
                    // (A refresh the user asked for looks into all of them.)
                    if !req.force && !self.looks_into(d, &req.known) {
                        continue;
                    }
                    // Its config folders: looked for now and then while it has
                    // sessions, and every time it's looked into while it has none.
                    if due || !self.wsl_looked.get(d).is_some_and(|(_, sessions)| *sessions) {
                        self.wsl_sources.retain(|s| s.distro.as_deref() != Some(d));
                        self.wsl_sources.extend(registry::wsl_sources(d));
                    }
                    self.wsl_now.push(d.clone());
                }
                self.wsl_running = running;
            }
        }
        if !wsl {
            self.wsl_running.clear();
            self.wsl_sources.clear();
            self.wsl_looked.clear();
        }
        self.read(req, vscode_exe, archived)
    }

    /// Reads the session lists, transcripts and window names: Windows', and those
    /// of the distributions this pass may read (`wsl_now`).
    fn read(
        &mut self,
        req: &Request,
        vscode_exe: Option<PathBuf>,
        archived: Option<std::collections::HashSet<String>>,
    ) -> Snapshot {
        let known = &req.known;
        let windows = Source { distro: None, config_dir: self.config_dir.clone() };
        let listed_at_ms = crate::win::now_ms();
        let mut listed = registry::list(&windows).unwrap_or_default();
        let mut scanned: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for source in &self.wsl_sources {
            let distro = source.distro.clone().unwrap_or_default();
            if !self.wsl_now.contains(&distro) {
                continue;
            }
            match registry::list(source) {
                Some(l) => listed.extend(l),
                None => failed.push(distro.clone()),
            }
            if !scanned.contains(&distro) {
                scanned.push(distro);
            }
        }
        scanned.retain(|d| !failed.contains(d));
        for d in &self.wsl_now {
            // A list that couldn't be read for a moment says nothing new.
            let before = self.wsl_looked.get(d).is_some_and(|(_, sessions)| *sessions);
            let sessions = listed.iter().any(|l| l.distro.as_deref() == Some(d)) || (failed.contains(d) && before);
            self.wsl_looked.insert(d.clone(), (Instant::now(), sessions));
        }

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
        let stats = if req.transcripts {
            self.scanner.scan(&with_config)
        } else {
            // Switched off: the app dropped what it had, so everything read so far is
            // forgotten here too and reported anew once it's switched on again.
            self.scanner = Scanner::default();
            Vec::new()
        };

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

        let (gpg, gpg_check) = self.gpg_pass(req, known);
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
            gpg_check,
        }
    }
}

/// Whether a running distribution is looked into now: always while it has
/// sessions (listed when it was last looked into, `last`, or known to the app),
/// else only every [`WSL_QUIET_EVERY`], so that one nobody uses can stop.
fn look_now(last: Option<(Duration, bool)>, known_there: bool) -> bool {
    match last {
        None => true,
        Some((ago, sessions)) => sessions || known_there || ago >= WSL_QUIET_EVERY,
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
    let mut c = Collector::new(config_dir);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_distribution_without_sessions_is_left_alone_in_between() {
        let secs = Duration::from_secs;
        // Never looked into: now.
        assert!(look_now(None, false));
        // It had sessions, or the app still shows one of its: every pass.
        assert!(look_now(Some((secs(2), true)), false));
        assert!(look_now(Some((secs(2), false)), true));
        // None: not again before WSL had the time to stop it.
        assert!(!look_now(Some((secs(2), false)), false));
        assert!(!look_now(Some((secs(29), false)), false));
        assert!(look_now(Some((secs(30), false)), false));
    }

    /// A folder of its own in the temp folder, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!("trayward-collector-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn known(id: &str, distro: Option<&str>) -> Known {
        Known { id: id.into(), cwd: "/home/u/app".into(), distro: distro.map(str::to_string) }
    }

    /// Windows' config folder, and a distribution's two (root's and a user's), the
    /// latter only while this pass may read it.
    #[test]
    fn finds_the_config_folder_of_a_session() {
        let dir = TempDir::new("configs");
        let windows = dir.0.join("windows");
        let (root, user) = (dir.0.join("root"), dir.0.join("user"));
        let mut c = Collector::new(&windows);
        c.wsl_sources = vec![
            Source { distro: Some("Ubuntu".into()), config_dir: root.clone() },
            Source { distro: Some("Ubuntu".into()), config_dir: user.clone() },
            Source { distro: Some("Debian".into()), config_dir: dir.0.join("other") },
        ];
        assert_eq!(c.configs_for(None), std::slice::from_ref(&windows));
        // Not read in this pass: none.
        assert!(c.configs_for(Some("Ubuntu")).is_empty());
        assert_eq!(c.config_of("a", "/home/u/app", Some("Ubuntu")), None);
        c.wsl_now = vec!["Ubuntu".into()];
        assert_eq!(c.configs_for(Some("Ubuntu")), [root.clone(), user.clone()]);
        assert_eq!(c.config_of("w", r"c:\dev\app", None), Some(windows));
        // Not found in either: the first, and not looked for again for a while.
        assert_eq!(c.config_of("a", "/home/u/app", Some("Ubuntu")), Some(root.clone()));
        assert!(matches!(c.session_configs.get("a"), Some((None, _))));
        // Found in the user's: remembered.
        let transcript = user.join("projects").join("-home-u-app").join("b.jsonl");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, "{}\n").unwrap();
        assert_eq!(c.config_of("b", "/home/u/app", Some("Ubuntu")), Some(user.clone()));
        std::fs::remove_file(&transcript).unwrap();
        assert_eq!(c.config_of("b", "/home/u/app", Some("Ubuntu")), Some(user));
        // Looked for long ago: again.
        c.session_configs.insert("a".into(), (None, Instant::now().checked_sub(LOOK_AGAIN * 2).unwrap()));
        assert_eq!(c.config_of("a", "/home/u/app", Some("Ubuntu")), Some(root));
    }

    #[test]
    fn looks_into_a_distribution_while_it_has_sessions() {
        let mut c = Collector::new(Path::new("nowhere"));
        assert!(c.looks_into("Ubuntu", &[]));
        c.wsl_looked.insert("Ubuntu".into(), (Instant::now(), false));
        assert!(!c.looks_into("Ubuntu", &[known("x", Some("Debian")), known("y", None)]));
        assert!(c.looks_into("Ubuntu", &[known("x", Some("Ubuntu"))]));
        c.wsl_looked.insert("Ubuntu".into(), (Instant::now(), true));
        assert!(c.looks_into("Ubuntu", &[]));
    }

    /// A window's name, from the lock of a VS Code window (here: this test's
    /// process, which runs), kept for a while.
    #[test]
    fn names_the_window_of_a_session() {
        let dir = TempDir::new("names");
        let config = dir.0.join(".claude");
        let app = dir.0.join("my-app");
        std::fs::create_dir_all(config.join("ide")).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        let lock = serde_json::json!({ "pid": std::process::id(), "workspaceFolders": [app.display().to_string()] });
        std::fs::write(config.join(r"ide\1.lock"), lock.to_string()).unwrap();
        let mut c = Collector::new(&config);
        let cwd = app.join("src").display().to_string();
        assert_eq!(c.window_name(None, &cwd, None).as_deref(), Some("my-app"));
        assert_eq!(c.window_name(None, &cwd, Some("🧩 ${rootName}")).as_deref(), Some("my-app"), "kept");
        c.names.clear();
        assert_eq!(c.window_name(None, &cwd, Some("🧩 ${rootName}")).as_deref(), Some("🧩 my-app"));
        // A distribution this pass doesn't read: no locks, no name.
        assert_eq!(c.window_name(Some("Ubuntu"), "/home/u/app", None), None);
        // An old name is looked up again.
        let old = Instant::now().checked_sub(NAME_TTL * 2).unwrap();
        c.names.insert((None, cwd.clone()), (old, Some("old".into())));
        assert_eq!(c.window_name(None, &cwd, None).as_deref(), Some("my-app"));
    }

    /// The GPG key timer: nothing is run again before it's due.
    #[test]
    fn asks_about_the_gpg_key_only_when_due() {
        let mut c = Collector::new(Path::new("nowhere"));
        let req = |gpg, force| Request { known: Vec::new(), force, transcripts: false, wsl: false, gpg };
        // Off, looked up a moment ago.
        c.gpg_found = Some(Instant::now());
        c.gpg_checked = Some(Instant::now());
        assert_eq!(c.gpg_pass(&req(false, false), &[]), (None, None));
        assert!(c.gpg_checked.is_none());
        // On, asked a moment ago.
        c.gpg_checked = Some(Instant::now());
        assert_eq!(c.gpg_pass(&req(true, false), &[]), (None, None));
        assert!(c.gpg_found.is_none());
        assert!(c.gpg(false, &[]).is_none());
    }
    /// minute and a half, and prints which run and which were read. To see that a
    /// stopped one stays stopped: `cargo test wsl_on_this_pc -- --ignored --nocapture`,
    /// and `wsl --terminate <distribution>` while it runs.
    #[test]
    #[ignore]
    fn wsl_on_this_pc() {
        // No Windows sessions: a folder that doesn't exist.
        let nowhere = std::env::temp_dir().join(format!("trayward-collector-{}", std::process::id()));
        let mut c = Collector::new(&nowhere);
        let req = Request { known: Vec::new(), force: false, transcripts: false, wsl: true, gpg: false };
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(90) {
            let s = c.pass(&req);
            println!(
                "{:>3} s: running {:?}, read {:?}, sessions {}",
                started.elapsed().as_secs(),
                s.wsl_running,
                s.wsl_scanned,
                s.listed.len()
            );
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}
