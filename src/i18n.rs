//! Translations of the app's text: its files' German tables, on top of the
//! core's (`trayward_core::i18n`, which also holds the language setting).
//!
//! - [`t`] for fixed text: `t("Settings")`.
//! - [`f`] for text with values: `f("{0} needs you", &[&name])`. The German
//!   template may put the placeholders in another order.

use trayward_core::i18n::{self as core, German, Table};
pub use trayward_core::i18n::{Choice, set};

#[cfg(test)]
pub use trayward_core::i18n::take_untranslated;

/// The German tables of the app's files, merged.
const TABLES: &[Table] = &[
    crate::app::DE,
    crate::flyout::DE,
    crate::help::DE,
    crate::menu::DE,
    crate::settings_window::DE,
    crate::stats_window::DE,
    crate::taskbars::DE,
    crate::terminal::DE,
    crate::vscode::DE,
];

static GERMAN: German = German::new(TABLES);

/// `s` in the current language.
pub fn t(s: &'static str) -> &'static str {
    core::t_in(&GERMAN, s)
}

/// `s` in the current language, for text that isn't a literal (e.g. passed
/// through a helper). Prefer [`t`] where the text is a literal.
pub fn tr(s: &str) -> String {
    core::tr_in(&GERMAN, s)
}

/// `template` in the current language, with `{0}`, `{1}`, … replaced by `args`.
pub fn f(template: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    core::fill(t(template), args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trayward_core::i18n::testing::*;

    #[test]
    fn translations_keep_their_placeholders() {
        check_placeholders(TABLES);
    }

    #[test]
    fn each_text_is_translated_once() {
        // Here and in the core: the same text means the same.
        check_translated_once(&[TABLES, core::TABLES].concat());
    }

    #[test]
    fn every_text_in_the_code_has_a_translation() {
        check_code_translated(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &GERMAN);
    }
}
