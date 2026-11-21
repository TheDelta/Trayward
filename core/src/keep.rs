//! Kept statistics. Claude Code deletes transcripts after `cleanupPeriodDays`
//! (30 days by default), and with them their statistics. With "Keep
//! statistics" on (Settings → Sessions), Trayward saves each session's numbers
//! (what `stats` reads from its transcript, and its subagents' replies) as
//! JSON in `%LOCALAPPDATA%\Trayward\kept`, one file per session, and counts a
//! session from there once its transcript is gone.
//!
//! A kept file has the time its session was last written (its transcript or a
//! subagent's), so an unchanged session isn't saved again, and says how far the
//! transcript was read and which version counted it (`stats::PARSER`): the
//! statistics start from it and read only what came since, and a file from a
//! version that counted differently is made again while its transcript exists.
//! Switching the option off deletes the folder.

use crate::registry::Source;
use crate::stats::{self, AgentFile, Stats};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// A transcript still being written isn't saved yet (seconds since its last change).
const SETTLE_SECS: i64 = 5 * 60;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether kept statistics are saved and counted (the setting).
pub fn enabled() -> bool {
    // Tests run side by side: each switches it for its own thread.
    #[cfg(any(test, feature = "testing"))]
    {
        TEST_ENABLED.with(std::cell::Cell::get)
    }
    #[cfg(not(any(test, feature = "testing")))]
    {
        ENABLED.load(Ordering::Relaxed)
    }
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    #[cfg(any(test, feature = "testing"))]
    TEST_ENABLED.with(|e| e.set(on));
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    static TEST_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Points the tests of this thread at their own folder (and turns keeping on).
#[cfg(any(test, feature = "testing"))]
pub fn use_dir(dir: &Path) {
    TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.to_path_buf()));
}

/// Where kept statistics live. None in the demo, which keeps nothing.
fn base() -> Option<PathBuf> {
    #[cfg(any(test, feature = "testing"))]
    {
        TEST_DIR.with(|d| d.borrow().clone())
    }
    #[cfg(not(any(test, feature = "testing")))]
    {
        if crate::demo::active() {
            return None;
        }
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Trayward").join("kept"))
    }
}

/// The folder for one place sessions run: Windows, or a WSL distribution.
fn folder(source: &Source) -> Option<PathBuf> {
    let name = match &source.distro {
        None => "windows".to_string(),
        Some(d) => format!("wsl-{}", d.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '.', "_")),
    };
    Some(base()?.join(name))
}

/// The format of a kept file.
const VERSION: u32 = 2;

/// What is kept of one session. `version` and `parser` come first: see [`parser_of`].
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Kept {
    version: u32,
    /// `stats::PARSER` of the version that counted it.
    parser: u32,
    stats: Stats,
    /// The subagents' replies (tokens and models), for their share of the work.
    agents: Vec<Stats>,
    /// How far the transcript was read (bytes of whole lines), and a check of the
    /// bytes before (`stats::mark`): reading goes on from there.
    offset: u64,
    mark: u64,
    /// The transcripts `agents` came from, in the same order, as they were then.
    agent_files: Vec<AgentFile>,
}

/// A kept file as it's written: [`Kept`] without copying the statistics.
#[derive(Serialize)]
struct Keeping<'a> {
    version: u32,
    parser: u32,
    stats: &'a Stats,
    agents: Vec<Stats>,
    offset: u64,
    mark: u64,
    agent_files: Vec<&'a AgentFile>,
}

/// What to keep of a session (see [`store`]).
pub struct Saving<'a> {
    pub stats: &'a Stats,
    /// Its subagents' transcripts and what they say (only the replies are kept).
    pub agents: &'a [(AgentFile, Arc<Stats>)],
    pub offset: u64,
    pub mark: u64,
}

/// What was kept of a session whose transcript reading can go on from (see [`resume`]).
pub struct Saved {
    pub stats: Stats,
    pub agents: Vec<(AgentFile, Stats)>,
    pub offset: u64,
    pub mark: u64,
}

fn modified(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

/// The kept sessions of a place: id, kept file and when its transcript was last written.
pub fn list(source: &Source) -> Vec<(String, PathBuf, i64)> {
    if !enabled() {
        return Vec::new();
    }
    let Some(dir) = folder(source) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let id = p.file_stem()?.to_string_lossy().into_owned();
            Some((id, p.clone(), modified(&p)?))
        })
        .collect()
}

/// Kept files, read once.
type Loaded = (Arc<Stats>, Vec<Arc<Stats>>);

fn cache() -> &'static Mutex<HashMap<PathBuf, Loaded>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Loaded>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Releases what was read (see `stats::forget`).
pub fn forget() {
    cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}

/// A kept session's statistics and its subagents'.
pub fn load(path: &Path) -> Option<Loaded> {
    if let Some(l) = cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(path) {
        return Some(l.clone());
    }
    let kept: Kept = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let loaded = (Arc::new(kept.stats), kept.agents.into_iter().map(Arc::new).collect());
    cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(path.to_path_buf(), loaded.clone());
    Some(loaded)
}

/// The kept file of session `id` of `source`.
fn file(source: &Source, id: &str) -> Option<PathBuf> {
    Some(folder(source)?.join(format!("{id}.json")))
}

/// Which `stats::PARSER` counted a kept file, from its first bytes (the file
/// starts with `{"version":2,"parser":1,`); 0 for a file from before it was noted.
fn parser_of(path: &Path) -> u32 {
    use std::io::Read;
    let mut head = [0u8; 64];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    let head = String::from_utf8_lossy(head.get(..n).unwrap_or_default());
    let Some((_, rest)) = head.split_once("\"parser\":") else { return 0 };
    rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
}

/// The time the kept file of a session carries: when the session was last written.
pub fn written(source: &Source, id: &str) -> Option<i64> {
    modified(&file(source, id)?)
}

/// What was kept of session `id`, if reading its transcript can go on from it:
/// counted the way this version counts, and with the place reading stopped.
/// Not from the files read once (`load`): the caller takes it over and adds to it.
pub fn resume(source: &Source, id: &str) -> Option<Saved> {
    if !enabled() {
        return None;
    }
    let path = file(source, id)?;
    if parser_of(&path) != stats::PARSER {
        return None;
    }
    let kept: Kept = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if kept.parser != stats::PARSER || kept.offset == 0 {
        return None;
    }
    // Without their transcripts' names (or a different number of them), the subagents are read again.
    let agents = if kept.agent_files.len() == kept.agents.len() {
        kept.agent_files.into_iter().zip(kept.agents).collect()
    } else {
        Vec::new()
    };
    Some(Saved { stats: kept.stats, agents, offset: kept.offset, mark: kept.mark })
}

/// Keeps a session's statistics; `written` is when the session was last written
/// (`stats::stamp`). Whether it worked.
pub fn store(source: &Source, id: &str, written: i64, saving: &Saving) -> bool {
    if !enabled() {
        return false;
    }
    let Some(target) = file(source, id) else { return false };
    let keeping = Keeping {
        version: VERSION,
        parser: stats::PARSER,
        stats: saving.stats,
        agents: saving.agents.iter().map(|(_, s)| s.replies_only()).collect(),
        offset: saving.offset,
        mark: saving.mark,
        agent_files: saving.agents.iter().map(|(f, _)| f).collect(),
    };
    let placed = (|| {
        let json = serde_json::to_vec(&keeping).ok()?;
        std::fs::create_dir_all(target.parent()?).ok()?;
        // Written next to it first, so a half-written file never counts.
        let tmp = target.with_extension("tmp");
        let placed = (|| {
            std::fs::write(&tmp, json).ok()?;
            let file = std::fs::File::options().write(true).open(&tmp).ok()?;
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(written.max(0) as u64)).ok()?;
            drop(file);
            std::fs::rename(&tmp, &target).ok()
        })();
        if placed.is_none() {
            let _ = std::fs::remove_file(&tmp);
        }
        placed
    })();
    if placed.is_some() {
        cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&target);
    }
    placed.is_some()
}

/// Saves the statistics of every session that changed since it was last kept
/// (and isn't being written right now), and of every one a version kept that
/// counted differently. Returns how many were saved. Reads only what a
/// transcript got since it was kept, without filling the statistics' cache.
pub fn keep_all(roots: &[Source], now: i64) -> usize {
    if !enabled() {
        return 0;
    }
    let mut saved = 0;
    for source in roots {
        let Some(dir) = folder(source) else { continue };
        for (id, path) in stats::transcript_files(source) {
            // Switched off meanwhile (which deletes everything kept): nothing more is kept.
            if !enabled() {
                return saved;
            }
            let Some(written) = stats::stamp(&path) else { continue };
            let target = dir.join(format!("{id}.json"));
            let current = modified(&target) == Some(written) && parser_of(&target) == stats::PARSER;
            if now - written < SETTLE_SECS || current {
                continue;
            }
            // Nothing to keep in it as it is (no prompt, no reply): not read again until it changes.
            if empty().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&path) == Some(&written) {
                continue;
            }
            match stats::keep_session(source, &id, &path) {
                Some((true, _)) => saved += 1,
                Some((false, false)) => {
                    empty().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(path, written);
                }
                _ => {}
            }
        }
    }
    saved
}

/// Transcripts with nothing to keep, and when they were written.
fn empty() -> &'static Mutex<HashMap<PathBuf, i64>> {
    static EMPTY: OnceLock<Mutex<HashMap<PathBuf, i64>>> = OnceLock::new();
    EMPTY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Places with kept statistics that `roots` doesn't have: WSL distributions that
/// aren't running. Their files can't be read without starting them, but what was
/// kept of their sessions still counts (`Source::kept_only`).
pub fn stopped(roots: &[Source]) -> Vec<Source> {
    if !enabled() {
        return Vec::new();
    }
    let Some(base) = base() else { return Vec::new() };
    let have: Vec<PathBuf> = roots.iter().filter_map(folder).collect();
    let Ok(entries) = std::fs::read_dir(base) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().is_dir() && !have.contains(&e.path()))
        .filter_map(|e| Some(Source::kept_only(e.file_name().to_str()?.strip_prefix("wsl-")?)))
        .collect()
}

/// Deletes everything kept (when the option is switched off).
pub fn delete_all() {
    forget();
    if let Some(dir) = base() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kept file can't be made again once its transcript is deleted: one from
    /// before a field existed must still be read, with that field's default.
    #[test]
    fn reads_a_file_from_before_a_field_was_added() {
        let old = r#"{"version":1,"stats":{"title":"Fix","prompts":[{"at":5}],
            "replies":[{"at":6,"tokens":{"output":7}}],"calls":[{"at":6,"tool":"Read"}]}}"#;
        let kept: Kept = serde_json::from_str(old).unwrap();
        assert_eq!(kept.stats.title.as_deref(), Some("Fix"));
        assert_eq!((kept.stats.prompts[0].at, kept.stats.prompts[0].text.as_str()), (5, ""));
        assert_eq!((kept.stats.replies[0].tokens.output, kept.stats.replies[0].tokens.input), (7, 0));
        assert_eq!((kept.stats.calls[0].tool.as_str(), kept.stats.calls[0].added), ("Read", 0));
        assert!(kept.agents.is_empty());
        assert_eq!((kept.parser, kept.offset), (0, 0));
    }

    #[test]
    fn tells_which_version_counted_a_file() {
        let dir = std::env::temp_dir().join(format!("trayward-keep-head-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.json");
        let keeping = Keeping {
            version: VERSION,
            parser: 37,
            stats: &Stats::default(),
            agents: Vec::new(),
            offset: 5,
            mark: 6,
            agent_files: Vec::new(),
        };
        std::fs::write(&path, serde_json::to_vec(&keeping).unwrap()).unwrap();
        assert_eq!(parser_of(&path), 37);
        // From before it was noted, cut off, or missing: none.
        std::fs::write(&path, r#"{"version":1,"stats":{"title":"the \"parser\": 5"}}"#).unwrap();
        assert_eq!(parser_of(&path), 0);
        std::fs::write(&path, r#"{"version":2,"parser":"#).unwrap();
        assert_eq!(parser_of(&path), 0);
        assert_eq!(parser_of(&dir.join("none.json")), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
