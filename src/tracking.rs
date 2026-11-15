//! Session tracking is a set of hooks in Claude Code's user settings
//! (`~/.claude/settings.json`) that run this exe in hook mode. This module
//! adds, updates and removes exactly those entries and leaves everything
//! else in the file as it was.

use crate::hook;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Hook events we listen to, and whether each runs asynchronously.
/// Tool events fire on every tool call, so they must not slow Claude down;
/// the rest are rare and run in order, so the app sees them in sequence.
const EVENTS: &[(&str, bool)] = &[
    ("SessionStart", false),
    ("UserPromptSubmit", false),
    ("Notification", false),
    ("PostToolUse", true),
    ("PostToolUseFailure", true),
    ("Stop", false),
    ("SubagentStart", false),
    ("SubagentStop", false),
    ("SessionEnd", false),
];

/// Seconds Claude Code waits for the hook at most. It normally takes a few ms.
const TIMEOUT_SECS: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    On,
    /// Some of our hooks are there but not all, or they point to another exe.
    Outdated,
}

fn is_ours(handler: &Value) -> bool {
    handler["args"].as_array().is_some_and(|a| a.len() == 1 && (a[0] == hook::ARG || a[0] == hook::LEGACY_ARG))
}

fn handler(exe: &str, is_async: bool) -> Value {
    let mut h = json!({ "type": "command", "command": exe, "args": [hook::ARG], "timeout": TIMEOUT_SECS });
    if is_async {
        h["async"] = Value::Bool(true);
    }
    h
}

/// Our handlers under one event.
fn ours_in(settings: &Value, event: &str) -> Vec<Value> {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
        .filter(|h| is_ours(h))
        .cloned()
        .collect()
}

pub fn state(settings: &Value, exe: &str) -> State {
    let mut any = false;
    let mut all = true;
    for &(event, is_async) in EVENTS {
        let ours = ours_in(settings, event);
        any |= !ours.is_empty();
        all &= ours.len() == 1 && ours[0] == handler(exe, is_async);
    }
    // Leftovers under events we no longer use also make it outdated.
    let stray = settings["hooks"]
        .as_object()
        .is_some_and(|m| m.keys().any(|k| !EVENTS.iter().any(|(e, _)| e == k) && !ours_in(settings, k).is_empty()));
    match (any || stray, all && !stray) {
        (_, true) => State::On,
        (false, _) => State::Off,
        (true, false) => State::Outdated,
    }
}

/// Removes all of our handlers. Groups, events and the `hooks` object that
/// only became empty because of that are dropped too; anything that was
/// already empty is the user's and stays.
pub fn remove(settings: &mut Value) {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else { return };
    let mut emptied_events = Vec::new();
    for (event, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else { continue };
        let before = groups.len();
        groups.retain_mut(|group| {
            let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) else { return true };
            let n = list.len();
            list.retain(|h| !is_ours(h));
            !(list.is_empty() && n > 0)
        });
        if groups.is_empty() && before > 0 {
            emptied_events.push(event.clone());
        }
    }
    for event in &emptied_events {
        hooks.shift_remove(event);
    }
    if hooks.is_empty() && !emptied_events.is_empty() {
        if let Some(obj) = settings.as_object_mut() {
            obj.shift_remove("hooks");
        }
    }
}

/// Adds our handlers (replacing any older ones of ours).
pub fn add(settings: &mut Value, exe: &str) {
    remove(settings);
    if !settings.is_object() {
        *settings = Value::Object(Map::new());
    }
    let hooks = settings
        .as_object_mut()
        .expect("settings is an object")
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    if !hooks.is_object() {
        *hooks = Value::Object(Map::new());
    }
    let hooks = hooks.as_object_mut().expect("hooks is an object");
    for &(event, is_async) in EVENTS {
        let groups = hooks.entry(event).or_insert_with(|| Value::Array(Vec::new()));
        if !groups.is_array() {
            *groups = Value::Array(Vec::new());
        }
        groups.as_array_mut().expect("event is an array").push(json!({ "hooks": [handler(exe, is_async)] }));
    }
}

// ---------- the settings file ----------

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

fn read(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        // Some editors put a byte order mark first.
        Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{} isn't valid JSON ({e}). Fix it first; it was left unchanged.", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(crate::i18n::f("Can't read {0}: {1}", &[&path.display(), &e])),
    }
}

/// Writes through a temporary file and a rename, so Claude Code never
/// reads a half-written file.
fn write(path: &Path, value: &Value) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    let tmp = path.with_extension(format!("json.ctb-{}.tmp", std::process::id()));
    // On disk before it replaces the settings: a power cut right after the
    // rename must not leave an empty file.
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        std::io::Write::write_all(&mut f, text.as_bytes())?;
        f.sync_all()
    });
    written.and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        crate::i18n::f("Can't write {0}: {1}", &[&path.display(), &e])
    })
}

pub fn exe_path() -> Result<String, String> {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .map_err(|e| crate::i18n::f("Can't find this app's exe: {0}", &[&e]))
}

pub fn current(config_dir: &Path) -> State {
    match (read(&settings_path(config_dir)), exe_path()) {
        (Ok(v), Ok(exe)) => state(&v, &exe),
        _ => State::Off,
    }
}

/// Turns tracking on or off by editing the settings file.
pub fn set(config_dir: &Path, on: bool) -> Result<(), String> {
    let path = settings_path(config_dir);
    // The file itself, where the settings are a link to it (dotfiles): the
    // rename would otherwise put a plain file in the link's place.
    let linked = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
    let path = if linked { std::fs::canonicalize(&path).unwrap_or(path) } else { path };
    std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    // Read, change, write; again if something else changed the file meanwhile.
    for _ in 0..3 {
        let before = read(&path)?;
        let mut v = before.clone();
        if let Some(what) = unexpected(&v) {
            return Err(format!("{}: {what}; it was left unchanged.", path.display()));
        }
        if on {
            add(&mut v, &exe_path()?);
        } else {
            remove(&mut v);
        }
        if v == before {
            return Ok(());
        }
        if read(&path)? == before {
            return write(&path, &v);
        }
    }
    Err(format!("{} keeps changing; it was left unchanged.", path.display()))
}

/// What in the settings isn't shaped as Claude Code writes it, where our
/// entries go. Such a file is the user's to fix: nothing of theirs is replaced.
fn unexpected(settings: &Value) -> Option<String> {
    if !settings.is_object() {
        return Some("it doesn't contain a JSON object".into());
    }
    let hooks = &settings["hooks"];
    if hooks.is_null() {
        return None;
    }
    let Some(hooks) = hooks.as_object() else { return Some("\"hooks\" isn't an object".into()) };
    EVENTS
        .iter()
        .find(|(event, _)| hooks.get(*event).is_some_and(|groups| !groups.is_array()))
        .map(|(event, _)| format!("\"hooks.{event}\" isn't a list"))
}

/// German translations of its messages (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("Can't read {0}: {1}", "{0} lässt sich nicht lesen: {1}"),
    ("Can't write {0}: {1}", "{0} lässt sich nicht schreiben: {1}"),
    ("Can't find this app's exe: {0}", "Die Programmdatei dieser App wurde nicht gefunden: {0}"),
];

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "C:\\apps\\trayward.exe";

    #[test]
    fn add_then_remove_restores_original() {
        let original = json!({
            "model": "opus",
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "notify-send done" }] }]
            },
            "permissions": { "allow": [] }
        });
        let mut v = original.clone();
        assert_eq!(state(&v, EXE), State::Off);
        add(&mut v, EXE);
        assert_eq!(state(&v, EXE), State::On);
        // The user's own Stop hook is untouched and still first.
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "notify-send done");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["PostToolUse"][0]["hooks"][0]["async"], true);
        assert_eq!(v["hooks"]["Stop"][1]["hooks"][0].get("async"), None);
        remove(&mut v);
        assert_eq!(v, original);
        // Key order is kept too.
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "hooks", "permissions"]);
    }

    #[test]
    fn remove_drops_empty_hooks_object() {
        let mut v = json!({ "a": 1 });
        add(&mut v, EXE);
        remove(&mut v);
        assert_eq!(v, json!({ "a": 1 }));
    }

    #[test]
    fn remove_keeps_users_own_empty_entries() {
        let original = json!({ "hooks": { "PreCompact": [], "Stop": [{ "hooks": [] }] } });
        let mut v = original.clone();
        add(&mut v, EXE);
        remove(&mut v);
        assert_eq!(v, original);
    }

    #[test]
    fn add_is_idempotent() {
        let mut once = json!({});
        add(&mut once, EXE);
        let mut twice = once.clone();
        add(&mut twice, EXE);
        assert_eq!(once, twice);
    }

    #[test]
    fn detects_moved_exe_and_partial_installs() {
        let mut v = json!({});
        add(&mut v, "C:\\old\\trayward.exe");
        assert_eq!(state(&v, EXE), State::Outdated);
        let mut partial = json!({});
        add(&mut partial, EXE);
        partial["hooks"].as_object_mut().unwrap().remove("Stop");
        assert_eq!(state(&partial, EXE), State::Outdated);
        let mut stray = json!({});
        add(&mut stray, EXE);
        stray["hooks"]["PreCompact"] = json!([{ "hooks": [handler(EXE, false)] }]);
        assert_eq!(state(&stray, EXE), State::Outdated);
        add(&mut stray, EXE);
        assert_eq!(state(&stray, EXE), State::On);
    }

    #[test]
    fn leaves_settings_it_does_not_understand_alone() {
        assert_eq!(unexpected(&json!({})), None);
        assert_eq!(unexpected(&json!({"hooks": {"Stop": [], "Other": "x"}})), None);
        assert!(unexpected(&json!([1])).is_some());
        assert!(unexpected(&json!({"hooks": "soon"})).unwrap().contains("hooks"));
        assert!(unexpected(&json!({"hooks": {"Stop": {"a": 1}}})).unwrap().contains("hooks.Stop"));
    }

    #[test]
    fn tolerates_odd_shapes() {
        let mut v = json!({ "hooks": { "Stop": "nonsense", "Foo": [{ "matcher": "x" }] } });
        add(&mut v, EXE);
        assert_eq!(state(&v, EXE), State::On);
        // Groups without a hooks list belong to the user and stay.
        assert_eq!(v["hooks"]["Foo"], json!([{ "matcher": "x" }]));
        remove(&mut v);
        assert_eq!(v["hooks"]["Foo"], json!([{ "matcher": "x" }]));
    }
}
