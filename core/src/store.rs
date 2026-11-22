//! What the app remembers across restarts: the last usage numbers and the
//! tracked sessions, in `%LOCALAPPDATA%\Trayward\state.json`.

use crate::sessions::Sessions;
use crate::usage::Usage;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Default, Serialize, Deserialize)]
pub struct Saved {
    pub usage: Option<Usage>,
    /// When `usage` was fetched (unix seconds).
    pub fetched_at: Option<i64>,
    /// Name of the subscription plan, e.g. "Ultra".
    #[serde(default)]
    pub plan: Option<String>,
    /// Readings of the weekly limit, for the daily budget and pace.
    #[serde(default)]
    pub history: Vec<crate::forecast::Sample>,
    #[serde(default)]
    pub sessions: Sessions,
    /// Limit warnings already shown: (which, the window's reset time).
    #[serde(default)]
    pub warned: Vec<(String, i64)>,
    /// Notifications are snoozed until then (unix seconds).
    #[serde(default)]
    pub snoozed_until: Option<i64>,
    /// The session pinned to the taskbar.
    #[serde(default)]
    pub pinned: Option<String>,
    /// The week (its Monday, unix seconds) the weekly summary was last made for.
    #[serde(default)]
    pub summary_week: Option<i64>,
    /// The weeks that are over, as they were summed up, the latest first.
    #[serde(default)]
    pub weeks: Vec<crate::alerts::Week>,
    /// When the update check last asked GitHub (unix seconds).
    #[serde(default)]
    pub update_checked: Option<i64>,
    /// The newer version a bubble already told about.
    #[serde(default)]
    pub update_announced: Option<String>,
    /// The GPG agent's process id and when it unlocked the signing key.
    #[serde(default)]
    pub gpg_unlocked: Option<(u32, i64)>,
    /// The session mailbox's folder last found (not one set in the settings).
    #[serde(default)]
    pub mailbox_found: Option<String>,
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Points the tests of this thread (and the writers they start) at their own
/// folder: tests never read or write the real state.
#[cfg(any(test, feature = "testing"))]
pub fn use_dir(dir: &std::path::Path) {
    TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.to_path_buf()));
}

fn path() -> Option<PathBuf> {
    #[cfg(any(test, feature = "testing"))]
    {
        TEST_DIR.with(|d| d.borrow().as_ref().map(|d| d.join("state.json")))
    }
    #[cfg(not(any(test, feature = "testing")))]
    {
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(PathBuf::from(base).join("Trayward").join("state.json"))
    }
}

pub fn load() -> Saved {
    if crate::demo::active() {
        // Made-up usage history; nothing of the real state.
        let now = crate::os::now_ms() / 1000;
        return Saved {
            history: crate::demo::history(now),
            plan: Some(crate::demo::plan().into()),
            ..Default::default()
        };
    }
    let Some(path) = path() else { return Saved::default() };
    let Ok(bytes) = std::fs::read(&path) else { return Saved::default() };
    match parse(&bytes) {
        Some((saved, true)) => saved,
        found => {
            // Not all of it readable (cut off, or from a newer version): set
            // aside, not overwritten by the next save, so it can be recovered.
            // What could be read is used.
            let _ = std::fs::rename(&path, path.with_extension("bad.json"));
            found.map(|(saved, _)| saved).unwrap_or_default()
        }
    }
}

/// Reads a state file: whole, or else field by field, so one unreadable part
/// (a session's numbers) doesn't lose the rest (the history of the weekly
/// limit). `None` if it isn't JSON at all; the flag says whether all was read.
fn parse(bytes: &[u8]) -> Option<(Saved, bool)> {
    if let Ok(saved) = serde_json::from_slice::<Saved>(bytes) {
        return Some((saved, true));
    }
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    fn field<T: serde::de::DeserializeOwned + Default>(v: &serde_json::Value, key: &str) -> T {
        serde_json::from_value(v[key].clone()).unwrap_or_default()
    }
    let saved = Saved {
        usage: field(&v, "usage"),
        fetched_at: field(&v, "fetched_at"),
        plan: field(&v, "plan"),
        history: field(&v, "history"),
        sessions: field(&v, "sessions"),
        warned: field(&v, "warned"),
        snoozed_until: field(&v, "snoozed_until"),
        pinned: field(&v, "pinned"),
        summary_week: field(&v, "summary_week"),
        weeks: field(&v, "weeks"),
        update_checked: field(&v, "update_checked"),
        update_announced: field(&v, "update_announced"),
        gpg_unlocked: field(&v, "gpg_unlocked"),
        mailbox_found: field(&v, "mailbox_found"),
    };
    Some((saved, false))
}

/// Saves the state on its own thread, so the UI thread never waits for the
/// disk. Saves that follow each other closely (every hook event asks for one)
/// become one: the newest state is written a moment after the first request.
pub struct Writer(std::sync::mpsc::Sender<Request>);

enum Request {
    Save(Box<Saved>),
    /// Write what's waiting now and answer when it's on disk.
    Flush(std::sync::mpsc::Sender<()>),
}

/// How long a save waits for newer ones.
const SAVE_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

impl Writer {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Request>();
        #[cfg(any(test, feature = "testing"))]
        let dir = TEST_DIR.with(|d| d.borrow().clone());
        std::thread::spawn(move || {
            #[cfg(any(test, feature = "testing"))]
            TEST_DIR.with(|d| *d.borrow_mut() = dir);
            crate::os::background_thread();
            while let Ok(first) = rx.recv() {
                let (mut newest, mut waiting) = match first {
                    Request::Save(s) => (Some(s), None),
                    Request::Flush(done) => (None, Some(done)),
                };
                let until = std::time::Instant::now() + SAVE_AFTER;
                while waiting.is_none() {
                    match rx.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
                        Ok(Request::Save(s)) => newest = Some(s),
                        Ok(Request::Flush(done)) => waiting = Some(done),
                        Err(_) => break,
                    }
                }
                if let Some(s) = newest {
                    save(&s);
                }
                if let Some(done) = waiting {
                    let _ = done.send(());
                }
            }
        });
        Self(tx)
    }

    /// Saves `saved` soon.
    pub fn save(&self, saved: Saved) {
        let _ = self.0.send(Request::Save(Box::new(saved)));
    }

    /// Waits until everything asked for is on disk (when the app exits).
    pub fn flush(&self) {
        let (done, wait) = std::sync::mpsc::channel();
        if self.0.send(Request::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(std::time::Duration::from_secs(3));
        }
    }
}

/// Best effort: losing the cache only means starting empty next time.
fn save(saved: &Saved) {
    if crate::demo::active() {
        return;
    }
    let Some(path) = path() else { return };
    let Ok(bytes) = serde_json::to_vec(saved) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &path)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_what_it_can_read_of_a_damaged_state() {
        let ok = br#"{"history":[{"at":1,"used":5.0,"resets_at":null}],"warned":[["weekly80",9]]}"#;
        let (saved, whole) = parse(ok).unwrap();
        assert!(whole);
        assert_eq!((saved.history.len(), saved.warned.len()), (1, 1));
        // A session list it can't read: the history stays.
        let bad = br#"{"history":[{"at":1,"used":5.0,"resets_at":null}],"sessions":{"list":[{"id":7}]}}"#;
        let (saved, whole) = parse(bad).unwrap();
        assert!(!whole);
        assert_eq!(saved.history.len(), 1);
        assert_eq!(parse(b"cut off {").map(|(_, w)| w), None);
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trayward-store-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        use_dir(&dir);
        dir
    }

    #[test]
    fn nothing_is_read_or_written_without_a_folder() {
        // A test thread that set no folder of its own never reaches the real state.
        assert!(path().is_none());
        assert!(load().plan.is_none());
        save(&Saved { plan: Some("Max".into()), ..Default::default() });
    }

    #[test]
    fn saves_and_loads_the_state() {
        let dir = test_dir("saves");
        assert!(load().history.is_empty());
        let w = Writer::spawn();
        // Saves close together become one: the newest.
        w.save(Saved { plan: Some("Pro".into()), ..Default::default() });
        w.save(Saved { plan: Some("Max".into()), pinned: Some("abc".into()), ..Default::default() });
        w.flush();
        let saved = load();
        assert_eq!((saved.plan.as_deref(), saved.pinned.as_deref()), (Some("Max"), Some("abc")));
        assert!(!dir.join("state.json.tmp").exists());
        // A flush with nothing waiting answers too.
        w.flush();
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_a_damaged_state_aside() {
        let dir = test_dir("damaged");
        let file = dir.join("state.json");
        std::fs::write(&file, br#"{"plan":"Max","sessions":{"list":[{"id":7}]}}"#).unwrap();
        let saved = load();
        assert_eq!(saved.plan.as_deref(), Some("Max"));
        assert!(!file.exists() && dir.join("state.bad.json").is_file());
        // Not JSON at all: nothing of it, and set aside too.
        std::fs::write(&file, b"cut off {").unwrap();
        assert!(load().plan.is_none());
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_that_fails_leaves_nothing_behind() {
        let dir = test_dir("fails");
        // The state's folder is a file: nothing can be written into it.
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, b"").unwrap();
        use_dir(&blocked);
        save(&Saved::default());
        assert!(std::fs::read(&blocked).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
