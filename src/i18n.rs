//! Translations. English is the source: the code holds the English text, and
//! each file that shows text has a `DE` table of German translations next to
//! it, registered in [`TABLES`]. Text without a translation stays English.
//!
//! - [`t`] for fixed text: `t("Settings")`.
//! - [`f`] for text with values: `f("{0} needs you", &[&name])`. The German
//!   template may put the placeholders in another order.
//! - Dates and weekdays follow the language too ([`locale`]).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use windows::core::{PCWSTR, w};

/// The language setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Windows' display language (German if it's German, else English).
    System,
    English,
    German,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
}

/// 0: system, 1: English, 2: German.
static CHOICE: AtomicU8 = AtomicU8::new(0);

pub fn set(choice: Choice) {
    // Tests run in parallel: each test thread has its own language.
    #[cfg(test)]
    {
        TEST_CHOICE.with(|c| c.set(Some(choice)));
        return;
    }
    #[allow(unreachable_code)]
    CHOICE.store(
        match choice {
            Choice::System => 0,
            Choice::English => 1,
            Choice::German => 2,
        },
        Ordering::Relaxed,
    );
}

#[cfg(test)]
thread_local! {
    static TEST_CHOICE: std::cell::Cell<Option<Choice>> = const { std::cell::Cell::new(None) };
}

pub fn choice() -> Choice {
    #[cfg(test)]
    if let Some(c) = TEST_CHOICE.with(std::cell::Cell::get) {
        return c;
    }
    match CHOICE.load(Ordering::Relaxed) {
        1 => Choice::English,
        2 => Choice::German,
        _ => Choice::System,
    }
}

/// The language text is shown in.
pub fn lang() -> Lang {
    // Tests compare English text, whatever language Windows speaks.
    if cfg!(test) && choice() == Choice::System {
        return Lang::En;
    }
    match choice() {
        Choice::English => Lang::En,
        Choice::German => Lang::De,
        Choice::System => system(),
    }
}

/// Windows' display language, as far as Trayward speaks it.
fn system() -> Lang {
    static SYSTEM: OnceLock<Lang> = OnceLock::new();
    *SYSTEM.get_or_init(|| {
        // SAFETY: plain query.
        let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
        // The primary language is the low 10 bits; 0x07 is German.
        if id & 0x3FF == 0x07 { Lang::De } else { Lang::En }
    })
}

/// The locale for dates and times: the user's own with the system language,
/// else one that matches the chosen language.
pub fn locale() -> PCWSTR {
    match choice() {
        Choice::System => PCWSTR::null(),
        Choice::English => w!("en-GB"),
        Choice::German => w!("de-DE"),
    }
}

/// The German tables of all files, merged.
const TABLES: &[&[(&str, &str)]] = &[
    COMMON,
    crate::app::DE,
    crate::flyout::DE,
    crate::menu::DE,
    crate::settings_window::DE,
    crate::stats_window::DE,
    crate::taskbars::DE,
    crate::terminal::DE,
    crate::tracking::DE,
    crate::usage::DE,
    crate::vscode::DE,
];

/// Words used in several places.
#[rustfmt::skip]
const COMMON: &[(&str, &str)] = &[
    ("now", "jetzt"),
    ("Weekly", "Woche"),
    ("5-hour", "5 Stunden"),
    ("On", "An"),
    ("Off", "Aus"),
];

fn german() -> &'static HashMap<&'static str, &'static str> {
    static DE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    DE.get_or_init(|| TABLES.iter().flat_map(|t| t.iter().copied()).collect())
}

/// `s` in the current language.
pub fn t(s: &'static str) -> &'static str {
    match lang() {
        Lang::En => s,
        Lang::De => german().get(s).copied().unwrap_or(s),
    }
}

/// `s` in the current language, for text that isn't a literal (e.g. passed
/// through a helper). Prefer [`t`] where the text is a literal.
pub fn tr(s: &str) -> String {
    match lang() {
        Lang::En => s.to_string(),
        Lang::De => match german().get(s) {
            Some(d) => d.to_string(),
            None => {
                #[cfg(test)]
                UNTRANSLATED.with(|u| u.borrow_mut().push(s.to_string()));
                s.to_string()
            }
        },
    }
}

#[cfg(test)]
thread_local! {
    static UNTRANSLATED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests: what went through [`tr`] in German without a translation since
/// the last call (texts that aren't literals, which the scan of the code
/// can't see: card titles, descriptions, choices).
#[cfg(test)]
pub fn take_untranslated() -> Vec<String> {
    UNTRANSLATED.with(|u| std::mem::take(&mut *u.borrow_mut()))
}

/// `template` in the current language, with `{0}`, `{1}`, … replaced by `args`.
pub fn f(template: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    fill(t(template), args)
}

fn fill(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    // In one pass: a value that contains "{1}" itself (a title, a name) stays as it is.
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let arg = after.find('}').and_then(|close| {
            let i: usize = after[..close].parse().ok()?;
            Some((args.get(i)?, close))
        });
        match arg {
            Some((value, close)) => {
                out.push_str(&value.to_string());
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out + rest
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The placeholders of a template, sorted.
    fn placeholders(s: &str) -> Vec<String> {
        let mut found: Vec<String> = (0..10).map(|i| format!("{{{i}}}")).filter(|p| s.contains(p.as_str())).collect();
        found.sort();
        found
    }

    #[test]
    fn translations_keep_their_placeholders() {
        for table in TABLES {
            for (en, de) in *table {
                assert_eq!(placeholders(en), placeholders(de), "{en:?} → {de:?}");
                assert!(!de.is_empty(), "{en:?} has an empty translation");
            }
        }
    }

    #[test]
    fn each_text_is_translated_once() {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for table in TABLES {
            for (en, de) in *table {
                if let Some(other) = seen.insert(en, de) {
                    assert_eq!(other, *de, "{en:?} is translated twice, differently");
                }
            }
        }
    }

    /// The string literals passed to `t(…)` / `f(…)` in `src` (escapes resolved).
    fn translated_literals(src: &str) -> Vec<String> {
        let mut found = Vec::new();
        let bytes = src.as_bytes();
        for (i, _) in src.match_indices("(\"") {
            // `t(` or `f(`, alone or after `::`, not the end of a longer name.
            let name_at = i.checked_sub(1).filter(|&j| matches!(bytes[j], b't' | b'f'));
            let Some(j) = name_at else { continue };
            if j > 0 && (bytes[j - 1].is_ascii_alphanumeric() || bytes[j - 1] == b'_') {
                continue;
            }
            let mut text = String::new();
            let mut chars = src[i + 2..].chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => text.push('\n'),
                        Some(other) => text.push(other),
                        None => break,
                    },
                    c => text.push(c),
                }
            }
            found.push(text);
        }
        found
    }

    #[test]
    fn every_text_in_the_code_has_a_translation() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let de = german();
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("i18n.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            // Only the code, not its tests.
            let code = src.split("#[cfg(test)]\nmod tests").next().unwrap_or_default();
            for text in translated_literals(code) {
                if !de.contains_key(text.as_str()) {
                    missing.push(format!("{}: {text:?}", path.file_name().unwrap().to_string_lossy()));
                }
            }
        }
        assert!(missing.is_empty(), "no German translation for:\n{}", missing.join("\n"));
    }

    #[test]
    fn finds_translated_literals() {
        let src = r#"t("A") i18n::f("B {0}", &[&x]) gift("no") crate::i18n::t("C\n\"q\"")"#;
        assert_eq!(translated_literals(src), ["A", "B {0}", "C\n\"q\""]);
    }

    #[test]
    fn fills_in_values() {
        assert_eq!(fill("{1} before {0}", &[&"a", &2]), "2 before a");
        // A value that looks like a placeholder isn't filled in again.
        assert_eq!(fill("{0}: {1}", &[&"fix {1} and {0}", &"done"]), "fix {1} and {0}: done");
        // Braces that aren't placeholders stay.
        assert_eq!(fill("{x} {9} { {0}", &[&"a"]), "{x} {9} { a");
        assert_eq!(fill("ü {0} ö", &[&"ä"]), "ü ä ö");
    }
}
