//! Kept statistics. Claude Code deletes transcripts after `cleanupPeriodDays`
//! (30 days by default), and with them their statistics. With "Keep
//! statistics" on (Settings → Sessions), Trayward saves each session's numbers
//! (what `stats` reads from its transcript, and its subagents' replies) as
//! JSON in `%LOCALAPPDATA%\Trayward\kept`, one file per session, and counts a
//! session from there once its transcript is gone.
//!
//! A kept file has the transcript's modification time, so an unchanged
//! transcript isn't saved again. Switching the option off deletes the folder.

use crate::registry::Source;
use crate::stats::{self, Stats};
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
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

#[cfg(test)]
thread_local! {
    static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Points the tests of this thread at their own folder (and turns keeping on).
#[cfg(test)]
pub fn use_dir(dir: &Path) {
    TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.to_path_buf()));
}

/// Where kept statistics live. None in the demo, which keeps nothing.
fn base() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_DIR.with(|d| d.borrow().clone())
    }
    #[cfg(not(test))]
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

/// What is kept of one session.
#[derive(Serialize, Deserialize)]
struct Kept {
    version: u32,
    stats: Stats,
    /// The subagents' replies (tokens and models), for their share of the work.
    agents: Vec<Stats>,
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
    cache().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// A kept session's statistics and its subagents'.
pub fn load(path: &Path) -> Option<Loaded> {
    if let Some(l) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        return Some(l.clone());
    }
    let kept: Kept = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let loaded = (Arc::new(kept.stats), kept.agents.into_iter().map(Arc::new).collect());
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), loaded.clone());
    Some(loaded)
}

/// Saves the statistics of every transcript that changed since it was last
/// kept (and isn't being written right now). Returns how many were saved.
/// Reads each transcript on its own, without filling the statistics' cache.
pub fn keep_all(roots: &[Source], now: i64) -> usize {
    if !enabled() {
        return 0;
    }
    let mut saved = 0;
    for source in roots {
        let Some(dir) = folder(source) else { continue };
        for (id, path) in stats::transcript_files(source) {
            let Some(written) = modified(&path) else { continue };
            let target = dir.join(format!("{id}.json"));
            if now - written < SETTLE_SECS || modified(&target) == Some(written) {
                continue;
            }
            // Nothing to keep in it as it is (no prompt, no reply): not read again until it changes.
            let mut empty = empty().lock().unwrap_or_else(|e| e.into_inner());
            if empty.get(&path) == Some(&written) {
                continue;
            }
            match save(&path, &target, written) {
                Some(true) => saved += 1,
                Some(false) => {
                    empty.insert(path, written);
                }
                None => {}
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

/// Keeps a transcript's statistics. `Some(false)`: there's nothing to keep in it.
fn save(transcript: &Path, target: &Path, written: i64) -> Option<bool> {
    let stats = stats::read_whole(transcript)?;
    if stats.prompts.is_empty() && stats.replies.is_empty() {
        return Some(false);
    }
    let agents = stats::subagent_files(transcript)
        .iter()
        .filter_map(|p| stats::read_whole(p))
        .map(Stats::replies_only)
        .collect();
    let json = serde_json::to_vec(&Kept { version: 1, stats, agents }).ok()?;
    std::fs::create_dir_all(target.parent()?).ok()?;
    // Written next to it first, so a half-written file never counts.
    let tmp = target.with_extension("tmp");
    let placed = (|| {
        std::fs::write(&tmp, json).ok()?;
        let file = std::fs::File::options().write(true).open(&tmp).ok()?;
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(written.max(0) as u64)).ok()?;
        drop(file);
        std::fs::rename(&tmp, target).ok()
    })();
    if placed.is_none() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    cache().lock().unwrap_or_else(|e| e.into_inner()).remove(target);
    Some(true)
}

/// Deletes everything kept (when the option is switched off).
pub fn delete_all() {
    forget();
    if let Some(dir) = base() {
        let _ = std::fs::remove_dir_all(dir);
    }
}
