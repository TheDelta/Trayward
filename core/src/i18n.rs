//! Translations. English is the source: the code holds the English text, and the
//! German for all of this crate's text is in one file, `i18n/de.rs` (another
//! language would get a file of its own next to it). Text without a translation
//! stays English.
//!
//! - [`t`] for fixed text: `t("Settings")`.
//! - [`f`] for text with values: `f("{0} needs you", &[&name])`. The German
//!   template may put the placeholders in another order.
//! - Dates and weekdays follow the language too (`crate::os`).
//!
//! The language is set here for everything. An app built on this crate has its
//! own tables in a [`German`] and looks text up with [`t_in`] and [`tr_in`],
//! which fall back to the tables here.

mod de;

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// The language setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The system's display language (German if it's German, else English).
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
    #[cfg(any(test, feature = "testing"))]
    {
        TEST_CHOICE.with(|c| c.set(Some(choice)));
        return;
    }
    #[allow(unreachable_code, reason = "in tests the language is set per thread, above")]
    CHOICE.store(
        match choice {
            Choice::System => 0,
            Choice::English => 1,
            Choice::German => 2,
        },
        Ordering::Relaxed,
    );
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static TEST_CHOICE: std::cell::Cell<Option<Choice>> = const { std::cell::Cell::new(None) };
}

pub fn choice() -> Choice {
    #[cfg(any(test, feature = "testing"))]
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
    // Tests compare English text, whatever language the system speaks.
    if cfg!(any(test, feature = "testing")) && choice() == Choice::System {
        return Lang::En;
    }
    match choice() {
        Choice::English => Lang::En,
        Choice::German => Lang::De,
        Choice::System => system(),
    }
}

/// The system's display language, as far as Trayward speaks it.
fn system() -> Lang {
    static SYSTEM: OnceLock<Lang> = OnceLock::new();
    *SYSTEM.get_or_init(|| if crate::os::system_language_is_german() { Lang::De } else { Lang::En })
}

/// A table of translations: (English, German).
pub type Table = &'static [(&'static str, &'static str)];

/// The German tables of this crate's files, merged.
pub const TABLES: &[Table] = &[de::DE];

/// German translations from a few tables, merged the first time one is looked up.
pub struct German {
    tables: &'static [Table],
    merged: OnceLock<HashMap<&'static str, &'static str>>,
}

impl German {
    pub const fn new(tables: &'static [Table]) -> Self {
        German { tables, merged: OnceLock::new() }
    }

    /// The German for `s`, if these tables have it.
    pub fn get(&self, s: &str) -> Option<&'static str> {
        self.merged.get_or_init(|| self.tables.iter().flat_map(|t| t.iter().copied()).collect()).get(s).copied()
    }

    /// Whether these tables have `s`.
    pub fn contains(&self, s: &str) -> bool {
        self.get(s).is_some()
    }
}

static GERMAN: German = German::new(TABLES);

/// `s` in the current language.
pub fn t(s: &'static str) -> &'static str {
    t_in(&GERMAN, s)
}

/// `s` in the current language, from `own` (an app's tables) or else this crate's.
pub fn t_in(own: &German, s: &'static str) -> &'static str {
    match lang() {
        Lang::En => s,
        Lang::De => own.get(s).or_else(|| GERMAN.get(s)).unwrap_or(s),
    }
}

/// `s` in the current language, for text that isn't a literal (e.g. passed
/// through a helper). Prefer [`t`] where the text is a literal.
pub fn tr(s: &str) -> String {
    tr_in(&GERMAN, s)
}

/// [`tr`] with `own` tables first, as [`t_in`].
pub fn tr_in(own: &German, s: &str) -> String {
    match lang() {
        Lang::En => s.to_string(),
        Lang::De => match own.get(s).or_else(|| GERMAN.get(s)) {
            Some(d) => d.to_string(),
            None => {
                #[cfg(any(test, feature = "testing"))]
                UNTRANSLATED.with(|u| u.borrow_mut().push(s.to_string()));
                s.to_string()
            }
        },
    }
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static UNTRANSLATED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests: what went through [`tr`] in German without a translation since
/// the last call (texts that aren't literals, which the scan of the code
/// can't see: card titles, descriptions, choices).
#[cfg(any(test, feature = "testing"))]
pub fn take_untranslated() -> Vec<String> {
    UNTRANSLATED.with(|u| std::mem::take(&mut *u.borrow_mut()))
}

/// `template` in the current language, with `{0}`, `{1}`, … replaced by `args`.
pub fn f(template: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    fill(t(template), args)
}

pub fn fill(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    // In one pass: a value that contains "{1}" itself (a title, a name) stays as it is.
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some((before, after)) = rest.split_once('{') {
        out.push_str(before);
        let arg = after.split_once('}').and_then(|(number, then)| {
            let i: usize = number.parse().ok()?;
            Some((args.get(i)?, then))
        });
        match arg {
            Some((value, then)) => {
                out.push_str(&value.to_string());
                rest = then;
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out + rest
}

/// Checks of the translations, for this crate's tests and an app's.
#[cfg(any(test, feature = "testing"))]
#[allow(clippy::unwrap_used, reason = "only for tests: a file that can't be read should stop them")]
pub mod testing {
    use super::{GERMAN, German, HashMap, Table};

    /// The placeholders of a template, sorted.
    pub fn placeholders(s: &str) -> Vec<String> {
        let mut found: Vec<String> = (0..10).map(|i| format!("{{{i}}}")).filter(|p| s.contains(p.as_str())).collect();
        found.sort();
        found
    }

    /// Every translation keeps its template's placeholders and isn't empty.
    pub fn check_placeholders(tables: &[Table]) {
        for table in tables {
            for (en, de) in *table {
                assert_eq!(placeholders(en), placeholders(de), "{en:?} → {de:?}");
                assert!(!de.is_empty(), "{en:?} has an empty translation");
            }
        }
    }

    /// No text is translated twice, differently.
    pub fn check_translated_once(tables: &[Table]) {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for table in tables {
            for (en, de) in *table {
                if let Some(other) = seen.insert(en, de) {
                    assert_eq!(other, *de, "{en:?} is translated twice, differently");
                }
            }
        }
    }

    /// Every text the code in `dir` passes to `t(…)` / `f(…)` (its tests left
    /// out) has a German translation in `own` or this crate's tables.
    pub fn check_code_translated(dir: &std::path::Path, own: &German) {
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("i18n.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            // Only the code, not its tests.
            let code = src.split("#[cfg(test)]\nmod tests").next().unwrap_or_default();
            for text in translated_literals(code) {
                if !own.contains(&text) && !GERMAN.contains(&text) {
                    missing.push(format!("{}: {text:?}", path.file_name().unwrap().to_string_lossy()));
                }
            }
        }
        assert!(missing.is_empty(), "no German translation for:\n{}", missing.join("\n"));
    }

    /// The string literals passed to `t(…)` / `f(…)` in `src` (escapes resolved).
    pub fn translated_literals(src: &str) -> Vec<String> {
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
            let mut chars = src.get(i + 2..).unwrap_or_default().chars();
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
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn translations_keep_their_placeholders() {
        check_placeholders(TABLES);
    }

    #[test]
    fn each_text_is_translated_once() {
        check_translated_once(TABLES);
    }

    #[test]
    fn every_text_in_the_code_has_a_translation() {
        // The OS modules (`os/`) aren't scanned: they show no text.
        check_code_translated(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &German::new(&[]));
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
