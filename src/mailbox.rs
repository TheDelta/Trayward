//! The session mailbox (opt-in, off by default): a folder where Claude Code
//! sessions that can't message each other (Windows and WSL) leave Markdown
//! messages for each other. See `docs/mailbox.md`.
//!
//! The layout, as the mailbox's own README sets it:
//! - `<root>/<project>/` is the inbox of the session working in the project
//!   folder of that name;
//! - `<inbox>/*.md` are unread messages, `<inbox>/done/*.md` handled ones;
//! - `<inbox>/.<name>.tmp` is a message being written (left out);
//! - a message is named `<UTC time>-<from project>-<topic>.md` and starts with
//!   `From:`, `To:`, `Date:` and `Re:` lines.
//!
//! Trayward only reads it: what's unread, since when, and what was handled.
//! Nothing is moved, written or sent anywhere.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

/// The mailbox folder's usual name, looked for above the sessions' folders.
const FOLDER: &str = ".claude-mailbox";
/// How often the mailbox is read.
const EVERY: Duration = Duration::from_secs(10);
/// Most unread messages read per inbox (the rest are counted).
const MAX_READ: usize = 50;

/// An unread message.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub path: PathBuf,
    /// Its sender's project, from `From:` (else the file name).
    pub from: Option<String>,
    /// What it's about, from the file name: "deploy image access".
    pub topic: String,
    /// When it was sent (unix seconds), from `Date:` or else the file's time.
    pub at: i64,
    /// It answers another message (`Re:`).
    pub reply: bool,
}

/// One session's inbox.
#[derive(Clone, Debug, PartialEq)]
pub struct Inbox {
    /// The project folder it belongs to, e.g. `shop`.
    pub name: String,
    pub path: PathBuf,
    /// Unread messages, oldest first (at most [`MAX_READ`]).
    pub unread: Vec<Message>,
    /// How many are unread in all.
    pub unread_count: usize,
    /// How many were handled (moved to `done/`).
    pub done: usize,
    /// When the last one was handled (unix seconds).
    pub last_done: Option<i64>,
}

impl Inbox {
    /// When the oldest unread message came.
    pub fn waiting_since(&self) -> Option<i64> {
        self.unread.first().map(|m| m.at)
    }

    /// The unread messages by sender: the sender, and its messages newest
    /// first; the senders with the most first.
    pub fn by_sender(&self) -> Vec<(String, Vec<&Message>)> {
        let mut groups: Vec<(String, Vec<&Message>)> = Vec::new();
        for m in self.unread.iter().rev() {
            let from = m.from.clone().unwrap_or_else(|| "?".into());
            match groups.iter_mut().find(|(f, _)| *f == from) {
                Some((_, list)) => list.push(m),
                None => groups.push((from, vec![m])),
            }
        }
        groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
        groups
    }
}

/// What the mailbox looks like now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// The mailbox folder in use: the one set in the settings, or the one found.
    pub root: Option<PathBuf>,
    /// The inboxes, those with unread messages first (the longest waiting first), then by name.
    pub inboxes: Vec<Inbox>,
}

impl Snapshot {
    pub fn unread(&self) -> usize {
        self.inboxes.iter().map(|i| i.unread_count).sum()
    }

    /// The inbox of the project folder `name`.
    pub fn inbox(&self, name: &str) -> Option<&Inbox> {
        self.inboxes.iter().find(|i| i.name.eq_ignore_ascii_case(name))
    }
}

/// The mailbox folder: `configured` if it's a folder; else a `.claude-mailbox`
/// in one of the folders above the sessions' (`hints`), then in the user's folder.
pub fn find_root(configured: Option<&Path>, hints: &[String]) -> Option<PathBuf> {
    if let Some(dir) = configured {
        return dir.is_dir().then(|| dir.to_path_buf());
    }
    let mut tried: Vec<PathBuf> = Vec::new();
    for hint in hints {
        let mut dir = Path::new(hint).parent();
        while let Some(d) = dir {
            let candidate = d.join(FOLDER);
            if !tried.contains(&candidate) {
                if candidate.is_dir() {
                    return Some(candidate);
                }
                tried.push(candidate);
            }
            dir = d.parent();
        }
    }
    let home = PathBuf::from(std::env::var_os("USERPROFILE")?).join(FOLDER);
    home.is_dir().then_some(home)
}

fn modified(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// The Markdown files directly in `dir`, without those being written (`.name.tmp`, hidden).
fn messages_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            !name.starts_with('.') && name.to_ascii_lowercase().ends_with(".md") && p.is_file()
        })
        .collect()
}

/// The value of a `Key: value` header line among the first lines of `text`.
fn header<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().take(12).find_map(|l| l.strip_prefix(key)?.strip_prefix(':')).map(str::trim).filter(|v| !v.is_empty())
}

/// A message's sender and topic from its file name `<time>-<from>-<topic>.md`
/// (`from` from its header helps where the project's name has dashes itself).
fn name_parts(stem: &str, from: Option<&str>) -> String {
    // The time: digits and a T, ending in Z.
    let rest = match stem.split_once('-') {
        Some((time, rest)) if time.ends_with('Z') && time.chars().next().is_some_and(|c| c.is_ascii_digit()) => rest,
        _ => stem,
    };
    let topic = match from {
        Some(f) => rest.strip_prefix(f).and_then(|r| r.strip_prefix('-')).unwrap_or(rest),
        None => rest.split_once('-').map_or(rest, |(_, t)| t),
    };
    let topic = topic.replace(['-', '_'], " ");
    let topic = topic.trim();
    if topic.is_empty() { stem.to_string() } else { topic.to_string() }
}

/// Reads one message: its header and what its file name says.
fn read_message(path: &Path) -> Message {
    // The header is at the start: no need to read a long message whole.
    let mut head = String::new();
    if let Ok(f) = std::fs::File::open(path) {
        use std::io::Read;
        let _ = f.take(2048).read_to_string(&mut head);
    }
    // The project name, without "(wsl)" behind it.
    let from = header(&head, "From").map(|f| f.split(" (").next().unwrap_or(f).trim().to_string());
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let at = header(&head, "Date").and_then(crate::timefmt::parse_rfc3339).or_else(|| modified(path)).unwrap_or(0);
    Message {
        path: path.to_path_buf(),
        topic: name_parts(&stem, from.as_deref()),
        from,
        at,
        reply: header(&head, "Re").is_some(),
    }
}

/// Reads the mailbox at `root`.
pub fn read(root: &Path) -> Snapshot {
    let mut inboxes = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !path.is_dir() {
            continue;
        }
        let files = messages_in(&path);
        let unread_count = files.len();
        let mut unread: Vec<Message> = files.iter().take(MAX_READ).map(|p| read_message(p)).collect();
        unread.sort_by_key(|m| m.at);
        let done_files = messages_in(&path.join("done"));
        let last_done = done_files.iter().filter_map(|p| modified(p)).max();
        inboxes.push(Inbox { name, path, unread, unread_count, done: done_files.len(), last_done });
    }
    // Waiting longest first, then the rest by name.
    inboxes.sort_by(|a, b| {
        (a.unread_count == 0)
            .cmp(&(b.unread_count == 0))
            .then(a.waiting_since().cmp(&b.waiting_since()))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Snapshot { root: Some(root.to_path_buf()), inboxes }
}

/// Reads the mailbox every few seconds while `on`, from the folder set in
/// `configured` or else one found above the folders in `hints` (the sessions'),
/// and posts a [`Snapshot`] to `ctl` as `msg` when it changed.
pub fn spawn(
    ctl: crate::win::SendHwnd,
    msg: u32,
    on: Arc<AtomicBool>,
    poke: Arc<AtomicBool>,
    configured: Arc<Mutex<Option<String>>>,
    hints: Arc<Mutex<Vec<String>>>,
) {
    std::thread::spawn(move || {
        crate::win::background_thread();
        let mut last: Option<Snapshot> = None;
        loop {
            // Asked to look now (switched on, another folder): it answers even
            // if nothing changed, so the settings know the search is over.
            let asked = poke.swap(false, Ordering::Relaxed);
            let snap = if on.load(Ordering::Relaxed) {
                let set = configured.lock().ok().and_then(|c| c.clone()).map(PathBuf::from);
                let hints = hints.lock().map(|h| h.clone()).unwrap_or_default();
                let root =
                    if crate::demo::active() { crate::demo::mailbox_dir() } else { find_root(set.as_deref(), &hints) };
                root.map(|r| read(&r)).unwrap_or_default()
            } else {
                Snapshot::default()
            };
            if asked || last.as_ref() != Some(&snap) {
                last = Some(snap.clone());
                crate::win::post_boxed(ctl, msg, snap);
            }
            // Until the next look, or until asked.
            let mut waited = Duration::ZERO;
            while waited < EVERY && !poke.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                waited += Duration::from_millis(100);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_file_name_says() {
        assert_eq!(name_parts("20261113T1430Z-game-deploy-image-access", Some("game")), "deploy image access");
        // A project with dashes in its name: the header tells where it ends.
        assert_eq!(name_parts("20261114T0557Z-shop-web-npm-audit", Some("shop-web")), "npm audit");
        assert_eq!(name_parts("20261114T0557Z-ops-dns", None), "dns");
        // Not as the README says: the name as it is.
        assert_eq!(name_parts("notes", None), "notes");
        // No topic after the sender: the rest of the name.
        assert_eq!(name_parts("20261114T0557Z-x", Some("x")), "x");
    }

    #[test]
    fn reads_a_header() {
        let text = "From: ops (wsl)\nTo: shop\nDate: 2026-11-14T06:05:11Z\nRe: 20261114T0557Z-a.md\n\nHello";
        assert_eq!(header(text, "From"), Some("ops (wsl)"));
        assert_eq!(header(text, "Re"), Some("20261114T0557Z-a.md"));
        assert_eq!(header("Date:\nFrom x", "Date"), None);
        assert_eq!(header("", "From"), None);
    }

    #[test]
    fn reads_a_mailbox() {
        let root = std::env::temp_dir().join(format!("trayward-mailbox-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write("README.md", "# the mailbox");
        write(
            "shop/20261114T0900Z-ops-dns-records.md",
            "From: ops (wsl)\nTo: shop\nDate: 2026-11-14T09:00:00Z\n\nText",
        );
        write(
            "shop/20261114T0800Z-api-token-rotation.md",
            "From: api (windows)\nTo: shop\nDate: 2026-11-14T08:00:00Z\nRe: 20261113T1200Z-shop-ask.md\n\nText",
        );
        // Being written, and not a message: left out.
        write("shop/.20261114T1000Z-api-x.md.tmp", "From: api");
        write("shop/notes.txt", "x");
        write("shop/done/20261113T1200Z-api-hello.md", "From: api\n");
        write("api/done/20261113T1300Z-shop-ask.md", "From: shop\n");
        write(".hidden/20261114T0700Z-x-y.md", "From: x\n");

        let snap = read(&root);
        assert_eq!(snap.inboxes.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["shop", "api"]);
        let web = snap.inbox("shop").unwrap();
        assert_eq!((web.unread_count, web.done), (2, 1));
        // Oldest first.
        assert_eq!(web.unread[0].topic, "token rotation");
        assert_eq!((web.unread[0].from.as_deref(), web.unread[0].reply), (Some("api"), true));
        assert_eq!(web.unread[1].topic, "dns records");
        assert_eq!(web.waiting_since(), crate::timefmt::parse_rfc3339("2026-11-14T08:00:00Z"));
        let api = snap.inbox("API").unwrap();
        assert_eq!((api.unread_count, api.done), (0, 1));
        assert!(api.last_done.is_some());
        assert_eq!(snap.unread(), 2);
        // By sender: one each, newest first.
        let senders: Vec<(String, usize)> = web.by_sender().into_iter().map(|(f, l)| (f, l.len())).collect();
        assert_eq!(senders, [("api".to_string(), 1), ("ops".to_string(), 1)]);

        // Found above a session's folder; a folder set in the settings wins.
        let project = root.parent().unwrap().join("some-project");
        let mailbox = root.parent().unwrap().join(FOLDER);
        let _ = std::fs::remove_dir_all(&mailbox);
        std::fs::create_dir_all(&mailbox).unwrap();
        assert_eq!(find_root(None, &[project.join("src").display().to_string()]), Some(mailbox.clone()));
        assert_eq!(find_root(Some(&root), &[project.display().to_string()]), Some(root.clone()));
        assert_eq!(find_root(Some(&root.join("missing")), &[]), None);
        let _ = std::fs::remove_dir_all(&mailbox);
        let _ = std::fs::remove_dir_all(&root);
    }
}
