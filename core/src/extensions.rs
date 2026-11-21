//! Your own extensions: information from programs of your own, shown in the
//! tooltip and the flyout. A program writes a JSON file into
//! `%LOCALAPPDATA%\Trayward\extensions\`; Trayward only reads it. Nothing in
//! the file is run, opened or sent anywhere: a file can only say what's shown
//! (text, a progress bar, a time), and only within limits. The format is in
//! `docs/extensions.md`; it's a contract (version 1).
//!
//! The folder is read when Windows tells of a change in it, and once a minute
//! besides (a change can go untold); files that didn't change aren't read again.

use crate::os::DirWatch;
use serde_json::Value as Json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// The file format's version this Trayward reads.
pub const VERSION: u64 = 1;
/// Most extension files read (in the order of their names).
pub const MAX_FILES: usize = 5;
/// A larger file isn't read.
pub const MAX_BYTES: u64 = 64 * 1024;
/// Most rows in the tooltip, and in the flyout, per extension.
pub const MAX_ROWS: usize = 8;
/// Longest text (a label, a value, the description), in characters; longer is cut.
pub const MAX_TEXT: usize = 100;
/// Longest id and label.
pub const MAX_NAME: usize = 40;

/// How often the folder is looked at when no change was told: a change can go
/// untold (and other systems tell none yet).
const EVERY: Duration = Duration::from_secs(60);
const EVERY_UNWATCHED: Duration = Duration::from_secs(5);
/// After a change, a moment for the writer to finish.
const SETTLE: Duration = Duration::from_millis(200);

/// What a row's value means for its colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// Nothing to say: the usual colour.
    #[default]
    Plain,
    /// Good (green).
    Ok,
    /// Needs a look (yellow).
    Warn,
    /// Failed (red).
    Bad,
}

/// What a row shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A bar filled to `fraction` (0 to 1), with `text` beside it (the
    /// percentage unless the file says something else).
    Progress {
        fraction: f64,
        text: String,
    },
    /// A moment (unix seconds): shown as how long until it, or since it.
    Time(i64),
    Text(String),
}

/// One row: a label and its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    pub value: Value,
    pub state: State,
}

impl Row {
    /// The value as text at `now` (unix seconds): a time as `in 2h 05m` or `5m ago`.
    pub fn text(&self, now: i64) -> String {
        match &self.value {
            Value::Progress { text, .. } | Value::Text(text) => text.clone(),
            Value::Time(at) if *at >= now => {
                crate::i18n::fill(crate::i18n::t("in {0}"), &[&crate::timefmt::countdown(at - now)])
            }
            Value::Time(at) => crate::i18n::fill(crate::i18n::t("{0} ago"), &[&crate::timefmt::ago(now - at)]),
        }
    }
}

/// An extension, as its file describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Extension {
    pub id: String,
    pub label: String,
    pub description: String,
    pub tooltip: Vec<Row>,
    pub flyout: Vec<Row>,
    /// After this many seconds without a change to the file, it's shown as out of date.
    pub stale_after: Option<i64>,
    /// When the file last changed (unix seconds).
    pub changed: i64,
}

impl Extension {
    /// How long ago its file changed, when that's longer than it said is fine.
    pub fn stale(&self, now: i64) -> Option<i64> {
        let age = now - self.changed;
        self.stale_after.filter(|limit| age > *limit).map(|_| age)
    }
}

/// A file in the folder: its extension, or why it can't be shown.
#[derive(Clone, Debug, PartialEq)]
pub struct File {
    /// The file's name, e.g. `backup.json`.
    pub name: String,
    pub read: Result<Extension, String>,
}

/// The folder now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// The folder looked at.
    pub dir: Option<PathBuf>,
    pub files: Vec<File>,
    /// Files past [`MAX_FILES`], not read.
    pub more: usize,
}

impl Snapshot {
    /// The extensions that could be read.
    pub fn extensions(&self) -> impl Iterator<Item = &Extension> {
        self.files.iter().filter_map(|f| f.read.as_ref().ok())
    }
}

/// The extensions' folder: `%LOCALAPPDATA%\Trayward\extensions` (the demo's own in the demo).
pub fn dir() -> Option<PathBuf> {
    if crate::demo::active() {
        return crate::demo::extensions_dir();
    }
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("Trayward").join("extensions"))
}

/// Text from a file, made safe to show: no control characters or
/// direction marks (they could make it read as something else), at most
/// `max` characters.
fn clean(s: &str, max: usize) -> String {
    let invisible =
        |c: char| matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}');
    let s: String =
        s.chars().filter(|c| !invisible(*c)).map(|c| if c.is_control() { ' ' } else { c }).collect::<String>();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        return s;
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// An id: lower-case letters, digits and hyphens.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_NAME
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The rows of `key` (`tooltip` or `flyout`); rows of a type this version
/// doesn't know are left out, so newer files still show what they can.
fn rows(v: &Json, key: &str) -> Result<Vec<Row>, String> {
    let Some(list) = v.get(key).filter(|l| !l.is_null()) else { return Ok(Vec::new()) };
    let list = list.as_array().ok_or_else(|| format!("\"{key}\" isn't a list"))?;
    if list.len() > MAX_ROWS {
        return Err(format!("\"{key}\" has {} rows; at most {MAX_ROWS}", list.len()));
    }
    let mut out = Vec::new();
    for (i, row) in list.iter().enumerate() {
        let at = || format!("\"{key}\" row {}", i + 1);
        if !row.is_object() {
            return Err(format!("{} isn't an object", at()));
        }
        let label = clean(row["label"].as_str().unwrap_or_default(), MAX_NAME);
        let state = match row["state"].as_str() {
            None => State::Plain,
            Some("ok") => State::Ok,
            Some("warn") => State::Warn,
            Some("bad") => State::Bad,
            Some(other) => return Err(format!("{}: unknown state \"{}\"", at(), clean(other, 20))),
        };
        let value = match row["type"].as_str() {
            Some("progress") => {
                let value = row["value"].as_f64().ok_or_else(|| format!("{}: \"value\" must be a number", at()))?;
                let max = match &row["max"] {
                    Json::Null => 100.0,
                    m => m.as_f64().filter(|m| *m > 0.0).ok_or_else(|| format!("{}: \"max\" must be above 0", at()))?,
                };
                let fraction = (value / max).clamp(0.0, 1.0);
                let text = match row["text"].as_str() {
                    Some(t) => clean(t, MAX_TEXT),
                    None => format!("{:.0} %", fraction * 100.0),
                };
                Value::Progress { fraction, text }
            }
            Some("time") => {
                let s = row["value"].as_str().unwrap_or_default();
                // With its time zone: without one, the time would be guessed.
                let zoned = s.ends_with(['Z', 'z'])
                    || s.get(s.len().saturating_sub(6)..)
                        .is_some_and(|z| z.starts_with(['+', '-']) && z.as_bytes().get(3) == Some(&b':'));
                let time = crate::timefmt::parse_rfc3339(s).filter(|_| zoned);
                Value::Time(
                    time.ok_or_else(|| format!("{}: \"value\" must be a time like 2026-11-18T23:00:00+01:00", at()))?,
                )
            }
            Some("text") => Value::Text(clean(
                row["value"].as_str().ok_or_else(|| format!("{}: \"value\" must be text", at()))?,
                MAX_TEXT,
            )),
            Some(_) => continue,
            None => return Err(format!("{}: no \"type\" (progress, time or text)", at())),
        };
        out.push(Row { label, value, state });
    }
    Ok(out)
}

/// Reads an extension's file; `changed` is when it last changed (unix seconds).
pub fn parse(text: &str, changed: i64) -> Result<Extension, String> {
    // Windows PowerShell 5 starts its UTF-8 files with a byte order mark.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let v: Json = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    if !v.is_object() {
        return Err("not a JSON object".into());
    }
    match v["version"].as_u64() {
        Some(VERSION) => {}
        Some(n) => return Err(format!("version {n}; this Trayward reads version {VERSION}")),
        None => return Err(format!("no \"version\" ({VERSION})")),
    }
    let id = v["id"].as_str().unwrap_or_default();
    if !valid_id(id) {
        return Err(format!("\"id\" must be 1 to {MAX_NAME} lower-case letters, digits or hyphens"));
    }
    let label = clean(v["label"].as_str().unwrap_or_default(), MAX_NAME);
    if label.is_empty() {
        return Err("no \"label\"".into());
    }
    let stale_after = match &v["stale_after"] {
        Json::Null => None,
        s => Some(
            s.as_i64().filter(|s| *s > 0).ok_or_else(|| "\"stale_after\" must be a number of seconds".to_string())?,
        ),
    };
    Ok(Extension {
        id: id.to_string(),
        label,
        description: clean(v["description"].as_str().unwrap_or_default(), MAX_TEXT),
        tooltip: rows(&v, "tooltip")?,
        flyout: rows(&v, "flyout")?,
        stale_after,
        changed,
    })
}

/// A file as last read, with what it was then (so it's read again only when it changed).
struct Known {
    modified: SystemTime,
    len: u64,
    read: Result<Extension, String>,
}

/// Reads the folder; `known` keeps the files read before.
fn read(dir: &Path, known: &mut HashMap<PathBuf, Known>) -> Snapshot {
    let mut paths: Vec<(PathBuf, std::fs::Metadata)> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))
                // Files only: not a link elsewhere.
                .filter_map(|p| std::fs::symlink_metadata(&p).ok().filter(std::fs::Metadata::is_file).map(|m| (p, m)))
                .collect()
        })
        .unwrap_or_default();
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    let more = paths.len().saturating_sub(MAX_FILES);
    paths.truncate(MAX_FILES);
    known.retain(|p, _| paths.iter().any(|(q, _)| q == p));
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut files = Vec::new();
    for (path, meta) in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let fresh = known.get(&path).is_some_and(|k| k.modified == modified && k.len == meta.len());
        if !fresh {
            let changed = modified.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            let read = if meta.len() > MAX_BYTES {
                Err(format!("larger than {} KB", MAX_BYTES / 1024))
            } else {
                read_limited(&path).and_then(|text| parse(&text, changed))
            };
            known.insert(path.clone(), Known { modified, len: meta.len(), read });
        }
        let mut read = known.get(&path).map_or_else(|| Err(String::new()), |k| k.read.clone());
        // Two files with one id: the first one's.
        if let Ok(e) = &read {
            if let Some(first) = ids.get(&e.id) {
                read = Err(format!("the id \"{}\" is already {first}'s", e.id));
            } else {
                ids.insert(e.id.clone(), name.clone());
            }
        }
        files.push(File { name, read });
    }
    Snapshot { dir: Some(dir.to_path_buf()), files, more }
}

/// A file's text, at most [`MAX_BYTES`] of it (it may have grown since it was measured).
fn read_limited(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("can't be read: {e}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("can't be read: {e}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("larger than {} KB", MAX_BYTES / 1024));
    }
    String::from_utf8(bytes).map_err(|_| "not UTF-8 text".into())
}

/// Reads the folder while `on`: when Windows tells of a change, and every
/// minute; `poke` asks for a look now (it answers even if nothing changed).
/// Switched off, nothing is read and the folder isn't watched.
pub fn spawn(send: impl Fn(Snapshot) + Send + 'static, on: Arc<AtomicBool>, poke: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        crate::os::background_thread();
        let mut known = HashMap::new();
        let mut watch: Option<DirWatch> = None;
        let mut last: Option<Snapshot> = None;
        let mut since = Duration::MAX;
        let mut changed = false;
        loop {
            let asked = poke.swap(false, Ordering::Relaxed);
            let snap = match dir().filter(|_| on.load(Ordering::Relaxed)) {
                Some(dir) => {
                    if watch.is_none() {
                        let _ = std::fs::create_dir_all(&dir);
                        watch = DirWatch::new(&dir);
                    }
                    let every = if watch.is_some() { EVERY } else { EVERY_UNWATCHED };
                    if changed {
                        // More changes while the writer finishes: one read after them.
                        std::thread::sleep(SETTLE);
                        while watch.as_ref().is_some_and(|w| w.wait(SETTLE)) {}
                    }
                    if asked || changed || since >= every || last.is_none() {
                        since = Duration::ZERO;
                        read(&dir, &mut known)
                    } else {
                        last.clone().unwrap_or_default()
                    }
                }
                None => {
                    watch = None;
                    known.clear();
                    since = Duration::MAX;
                    Snapshot::default()
                }
            };
            if asked || last.as_ref() != Some(&snap) {
                last = Some(snap.clone());
                send(snap);
            }
            // Until a change, the next look, or until asked.
            let step = Duration::from_millis(250);
            changed = match &watch {
                Some(w) => w.wait(step),
                None => {
                    std::thread::sleep(step);
                    false
                }
            };
            since = since.saturating_add(step);
        }
    });
}

#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("in {0}", "in {0}"),
    ("{0} ago", "vor {0}"),
];

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{
        "version": 1,
        "id": "nas-backup",
        "label": "Backup",
        "description": "Nightly backup of the NAS",
        "tooltip": [
            { "label": "Backup", "type": "progress", "value": 42 },
            { "label": "Next run", "type": "time", "value": "2026-11-18T23:00:00+01:00" },
            { "label": "Status", "type": "text", "value": "Copying photos" }
        ],
        "flyout": [
            { "label": "Shares", "type": "progress", "value": 3, "max": 5, "text": "3 of 5", "state": "ok" },
            { "label": "Last error", "type": "text", "value": "Share offline", "state": "bad", "link": "https://x" }
        ],
        "stale_after": 600
    }"#;

    #[test]
    fn reads_the_format() {
        let e = parse(FULL, 100).unwrap();
        assert_eq!((e.id.as_str(), e.label.as_str()), ("nas-backup", "Backup"));
        assert_eq!(e.description, "Nightly backup of the NAS");
        assert_eq!(e.tooltip.len(), 3);
        assert_eq!(e.tooltip[0].value, Value::Progress { fraction: 0.42, text: "42 %".into() });
        assert_eq!(e.tooltip[1].value, Value::Time(crate::timefmt::parse_rfc3339("2026-11-18T22:00:00Z").unwrap()));
        assert_eq!(e.tooltip[2].value, Value::Text("Copying photos".into()));
        assert_eq!(e.flyout[0].value, Value::Progress { fraction: 0.6, text: "3 of 5".into() });
        assert_eq!((e.flyout[0].state, e.flyout[1].state), (State::Ok, State::Bad));
        assert_eq!((e.stale_after, e.changed), (Some(600), 100));
    }

    #[test]
    fn a_byte_order_mark_and_missing_parts_are_fine() {
        let e = parse("\u{FEFF}{\"version\":1,\"id\":\"a\",\"label\":\"A\"}", 0).unwrap();
        assert!(e.tooltip.is_empty() && e.flyout.is_empty() && e.description.is_empty() && e.stale_after.is_none());
    }

    #[test]
    fn says_what_is_wrong() {
        let bad = |s: &str| parse(s, 0).unwrap_err();
        assert!(bad("not json").starts_with("not valid JSON"));
        assert!(bad("[]").contains("object"));
        assert!(bad(r#"{"id":"a","label":"A"}"#).contains("version"));
        assert!(bad(r#"{"version":2,"id":"a","label":"A"}"#).contains("version 2"));
        assert!(bad(r#"{"version":1,"id":"Not ok","label":"A"}"#).contains("\"id\""));
        assert!(bad(r#"{"version":1,"id":"a"}"#).contains("label"));
        assert!(bad(r#"{"version":1,"id":"a","label":"A","tooltip":{}}"#).contains("isn't a list"));
        let row = |r: &str| bad(&format!(r#"{{"version":1,"id":"a","label":"A","tooltip":[{r}]}}"#));
        assert!(row(r#"{"label":"x","type":"progress","value":"1"}"#).contains("number"));
        assert!(row(r#"{"label":"x","type":"progress","value":1,"max":0}"#).contains("max"));
        assert!(row(r#"{"label":"x","type":"time","value":"2026-11-18T23:00:00"}"#).contains("time like"));
        assert!(row(r#"{"label":"x","type":"text","value":5}"#).contains("text"));
        assert!(row(r#"{"label":"x","type":"text","value":"y","state":"red"}"#).contains("state"));
        assert!(row(r#"{"label":"x"}"#).contains("type"));
        let many = [r#"{"type":"text","value":"y"}"#; MAX_ROWS + 1].join(",");
        assert!(row(&many).contains("at most"));
    }

    #[test]
    fn newer_row_types_are_left_out() {
        let e = parse(
            r#"{"version":1,"id":"a","label":"A","tooltip":[{"type":"chart","value":[1]},{"type":"text","value":"y"}]}"#,
            0,
        )
        .unwrap();
        assert_eq!(e.tooltip.len(), 1);
    }

    #[test]
    fn text_is_cleaned_and_cut() {
        assert_eq!(clean("a\nb\t c\u{202E}d\u{200B}", 50), "a b cd");
        let long = "x".repeat(MAX_TEXT + 10);
        let cut = clean(&long, MAX_TEXT);
        assert_eq!(cut.chars().count(), MAX_TEXT);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn progress_stays_within_its_bar() {
        let e = parse(
            r#"{"version":1,"id":"a","label":"A","tooltip":[{"type":"progress","value":150},{"type":"progress","value":-5}]}"#,
            0,
        )
        .unwrap();
        assert_eq!(e.tooltip[0].value, Value::Progress { fraction: 1.0, text: "100 %".into() });
        assert_eq!(e.tooltip[1].value, Value::Progress { fraction: 0.0, text: "0 %".into() });
    }

    #[test]
    fn times_count_down_and_up() {
        let row = |at| Row { label: String::new(), value: Value::Time(at), state: State::Plain };
        assert_eq!(row(1000 + 2 * 3600 + 5 * 60).text(1000), "in 2h 05m");
        assert_eq!(row(1000 - 5 * 60).text(1000), "5m ago");
    }

    #[test]
    fn stale_after_its_time() {
        let e = parse(FULL, 1000).unwrap();
        assert_eq!(e.stale(1000 + 600), None);
        assert_eq!(e.stale(1000 + 601), Some(601));
    }

    #[test]
    fn reads_a_folder_once_per_change() {
        let dir = std::env::temp_dir().join(format!("trayward-extensions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        write("a.json", r#"{"version":1,"id":"same","label":"A"}"#);
        write("b.json", r#"{"version":1,"id":"same","label":"B"}"#);
        write("c.json", "{");
        write("notes.txt", "not an extension");
        write("d.json", &format!(r#"{{"version":1,"id":"d","label":"{}"}}"#, "x".repeat(MAX_BYTES as usize)));
        let mut known = HashMap::new();
        let snap = read(&dir, &mut known);
        let names: Vec<&str> = snap.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a.json", "b.json", "c.json", "d.json"]);
        assert_eq!(snap.files[0].read.as_ref().map(|e| e.label.as_str()), Ok("A"));
        assert!(snap.files[1].read.as_ref().unwrap_err().contains("a.json"));
        assert!(snap.files[2].read.as_ref().unwrap_err().contains("JSON"));
        assert!(snap.files[3].read.as_ref().unwrap_err().contains("KB"));
        assert_eq!(snap.extensions().count(), 1);
        for n in 0..3 {
            write(&format!("e{n}.json"), "{}");
        }
        let snap = read(&dir, &mut known);
        assert_eq!((snap.files.len(), snap.more), (MAX_FILES, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
