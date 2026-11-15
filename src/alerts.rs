//! When the limit bubbles come and what the taskbar counts down: the
//! decisions, apart from the app so they can be tested.
//!
//! - Warnings: the weekly limit at the chosen level and at 95 %, the 5-hour
//!   limit at its level, and a pace that would use up the week early; each
//!   once per limit window.
//! - Free again: a limit that was at 80 % or more has just reset.
//! - Countdown: the time until a limit resets, once it's at the chosen level.

use crate::forecast::{self, Sample};
use crate::i18n;
use crate::timefmt;
use crate::usage::{Limit, Usage};

/// The start of the local week (Monday 00:00) that `t` is in, as unix seconds;
/// `offset` is the local time's offset from UTC in seconds.
pub fn week_start(t: i64, offset: i64) -> i64 {
    let day = (t + offset).div_euclid(86_400);
    // 1970-01-01 was a Thursday: Monday is 3 days later in the week.
    let monday = day - (day + 3).rem_euclid(7);
    monday * 86_400 - offset
}

/// Last week, as the weekly summary tells it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Week {
    pub prompts: u32,
    /// Tokens Claude wrote.
    pub tokens: u64,
    /// What the tokens would cost at API prices (dollars).
    pub cost: f64,
    /// How long Claude worked (seconds).
    pub working: i64,
}

/// The weekly summary's headline and detail; None for a week without prompts.
pub fn week_text(w: &Week) -> Option<(String, String)> {
    (w.prompts > 0).then(|| {
        (
            i18n::f("Your week: {0} prompts, {1} tokens", &[&w.prompts, &crate::transcripts::tokens(w.tokens)]),
            i18n::f(
                "{0} API value · Claude worked {1}",
                &[&crate::stats::dollars(w.cost), &timefmt::countdown(w.working)],
            ),
        )
    })
}

/// A limit warning that is due.
#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    /// Which warning ("weekly80", "5h90", "pace"); with `reset`, it shows once.
    pub key: String,
    /// The window it's about (its reset time).
    pub reset: i64,
    pub headline: String,
    pub detail: String,
}

/// The warning levels (Settings → Limits); 0 is off.
#[derive(Clone, Copy, Debug)]
pub struct Levels {
    pub weekly: u8,
    pub five_hour: u8,
    pub pace: bool,
}

/// The warnings due at `t`, the most urgent first.
pub fn warnings(u: &Usage, levels: Levels, history: &[Sample], t: i64) -> Vec<Warning> {
    let mut due = Vec::new();
    if let Some(w) = u.weekly {
        let used = w.used_at(t);
        if let Some(reset) = w.resets_at {
            let when = timefmt::local_time(reset, true);
            let steps: &[u8] = match levels.weekly {
                0 => &[],
                l if l < 95 => &[95, l],
                _ => &[95],
            };
            for &th in steps {
                if used >= f64::from(th) {
                    due.push(Warning {
                        key: format!("weekly{th}"),
                        reset,
                        headline: i18n::f("Weekly limit at {0} %", &[&format!("{used:.0}")]),
                        detail: i18n::f("Resets {0}", &[&when]),
                    });
                }
            }
            let plan = forecast::plan(history, used, reset, t);
            if let (Some(full), true) = (plan.full_at, used < 95.0 && levels.pace) {
                due.push(Warning {
                    key: "pace".into(),
                    reset,
                    headline: i18n::t("The weekly limit could run out early").into(),
                    detail: i18n::f(
                        "At this pace, around {0} (resets {1})",
                        &[&timefmt::local_time(full, true), &when],
                    ),
                });
            }
        }
    }
    if let (Some(f), level) = (u.five_hour, levels.five_hour) {
        let used = f.used_at(t);
        if let (true, Some(reset)) = (level > 0 && used >= f64::from(level), f.resets_at) {
            due.push(Warning {
                key: format!("5h{level}"),
                reset,
                headline: i18n::f("5-hour limit at {0} %", &[&format!("{used:.0}")]),
                detail: i18n::f("Resets {0}", &[&timefmt::local_time(reset, false)]),
            });
        }
    }
    due
}

/// Whether this warning was shown for this window already (reset times can
/// shift a little between readings).
fn shown(warned: &[(String, i64)], key: &str, reset: i64) -> bool {
    warned.iter().any(|(k, r)| k == key && (r - reset).abs() < 3600)
}

/// The first due warning not shown yet. Everything due counts as shown from
/// now on, so a lower step doesn't follow a higher one.
pub fn take_fresh(due: Vec<Warning>, warned: &mut Vec<(String, i64)>) -> Option<Warning> {
    let fresh: Vec<Warning> = due.into_iter().filter(|w| !shown(warned, &w.key, w.reset)).collect();
    for w in &fresh {
        warned.push((w.key.clone(), w.reset));
    }
    fresh.into_iter().next()
}

/// A limit that was nearly used up and has just reset (within ten minutes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freed {
    pub key: &'static str,
    pub weekly: bool,
    pub reset: i64,
}

/// The limits that are free again at `t` and weren't announced yet.
pub fn freed(u: &Usage, warned: &[(String, i64)], t: i64) -> Vec<Freed> {
    let mut out = Vec::new();
    for (key, weekly, limit) in [("free-weekly", true, u.weekly), ("free-5h", false, u.five_hour)] {
        let Some(reset) = limit.filter(|l| l.used >= 80.0).and_then(|l| l.resets_at) else { continue };
        if reset <= t && t - reset < 600 && !warned.iter().any(|(k, r)| k == key && *r == reset) {
            out.push(Freed { key, weekly, reset });
        }
    }
    out
}

/// The bubble's headline for limits that are free again.
pub fn freed_headline(freed: &[Freed]) -> &'static str {
    match freed {
        [one] if one.weekly => i18n::t("The weekly limit is free again"),
        [_] => i18n::t("The 5-hour limit is free again"),
        _ => i18n::t("Weekly and 5-hour limits are free again"),
    }
}

/// The time until each limit resets (e.g. "2h 10m"), once it's used to the
/// level `at` (0: never); whole minutes, so the widget redraws once a minute.
pub fn countdowns(limits: [Option<Limit>; 2], at: u8, t: i64) -> [Option<String>; 2] {
    limits.map(|l| {
        let l = l?;
        let reset = l.resets_at?;
        (at > 0 && l.used_at(t) >= f64::from(at) && reset > t).then(|| timefmt::countdown(reset - t))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_794_490_000;
    const HOUR: i64 = 3600;

    fn usage(weekly: Option<f64>, five_hour: Option<f64>) -> Usage {
        Usage {
            weekly: weekly.map(|used| Limit { used, resets_at: Some(T + 50 * HOUR) }),
            five_hour: five_hour.map(|used| Limit { used, resets_at: Some(T + 2 * HOUR) }),
        }
    }

    const LEVELS: Levels = Levels { weekly: 80, five_hour: 90, pace: false };

    fn keys(w: &[Warning]) -> Vec<&str> {
        w.iter().map(|w| w.key.as_str()).collect()
    }

    #[test]
    fn warns_at_the_levels() {
        assert!(warnings(&usage(Some(79.0), Some(89.0)), LEVELS, &[], T).is_empty());
        assert_eq!(keys(&warnings(&usage(Some(85.0), None), LEVELS, &[], T)), ["weekly80"]);
        assert_eq!(keys(&warnings(&usage(Some(96.0), Some(91.0)), LEVELS, &[], T)), ["weekly95", "weekly80", "5h90"]);
        let w = &warnings(&usage(Some(85.0), None), LEVELS, &[], T)[0];
        assert_eq!(w.headline, "Weekly limit at 85 %");
        // Off, and a level of 95 (no second step).
        let off = Levels { weekly: 0, five_hour: 0, pace: false };
        assert!(warnings(&usage(Some(99.0), Some(99.0)), off, &[], T).is_empty());
        let high = Levels { weekly: 95, ..LEVELS };
        assert_eq!(keys(&warnings(&usage(Some(97.0), None), high, &[], T)), ["weekly95"]);
    }

    #[test]
    fn each_warning_shows_once_per_window() {
        let mut warned = Vec::new();
        let first = take_fresh(warnings(&usage(Some(96.0), None), LEVELS, &[], T), &mut warned);
        assert_eq!(first.map(|w| w.key), Some("weekly95".into()));
        // Both steps count as shown: 80 % doesn't follow.
        assert!(take_fresh(warnings(&usage(Some(96.0), None), LEVELS, &[], T + 60), &mut warned).is_none());
        // The next window warns again.
        let next = Usage { weekly: Some(Limit { used: 85.0, resets_at: Some(T + 220 * HOUR) }), five_hour: None };
        assert!(take_fresh(warnings(&next, LEVELS, &[], T), &mut warned).is_some());
    }

    #[test]
    fn warns_about_the_pace() {
        // 60 % used two days into the week: the week runs out early.
        let reset = T + 5 * 24 * HOUR;
        let history: Vec<Sample> = (0..=48)
            .map(|h| Sample {
                at: T - (48 - h) * HOUR,
                used: 60.0 * h as f64 / 48.0,
                resets_at: Some(reset),
                plan: None,
            })
            .collect();
        let u = Usage { weekly: Some(Limit { used: 60.0, resets_at: Some(reset) }), five_hour: None };
        assert!(warnings(&u, LEVELS, &history, T).is_empty()); // pace warnings off
        let on = Levels { pace: true, ..LEVELS };
        assert_eq!(keys(&warnings(&u, on, &history, T)), ["pace"]);
    }

    #[test]
    fn tells_when_a_limit_is_free_again() {
        let reset = T - 120;
        let full = Usage { weekly: None, five_hour: Some(Limit { used: 97.0, resets_at: Some(reset) }) };
        let found = freed(&full, &[], T);
        assert_eq!(found, [Freed { key: "free-5h", weekly: false, reset }]);
        assert_eq!(freed_headline(&found), "The 5-hour limit is free again");
        // Announced already, too long ago, or never nearly used up: nothing.
        assert!(freed(&full, &[("free-5h".into(), reset)], T).is_empty());
        assert!(freed(&full, &[], T + 3600).is_empty());
        let low = Usage { weekly: None, five_hour: Some(Limit { used: 50.0, resets_at: Some(reset) }) };
        assert!(freed(&low, &[], T).is_empty());
        let both = [Freed { key: "free-weekly", weekly: true, reset }, found[0]];
        assert_eq!(freed_headline(&both), "Weekly and 5-hour limits are free again");
    }

    #[test]
    fn finds_the_week() {
        // Thursday 2026-11-12 10:00 UTC; Monday 2026-11-09 00:00 UTC.
        let thursday = 1_794_477_600;
        let monday = 1_794_182_400;
        assert_eq!(week_start(thursday, 0), monday);
        assert_eq!(week_start(monday, 0), monday);
        // Sunday 23:00 local is still the week before.
        assert_eq!(week_start(monday - 3600, 0), monday - 7 * 86_400);
        // An hour east of UTC: local midnight is an hour earlier.
        assert_eq!(week_start(thursday, 3600), monday - 3600);
    }

    #[test]
    fn sums_up_the_week() {
        assert!(week_text(&Week::default()).is_none());
        let w = Week { prompts: 74, tokens: 843_000, cost: 50.55, working: 7 * 3600 + 48 * 60 };
        let (head, detail) = week_text(&w).unwrap();
        assert_eq!(head, "Your week: 74 prompts, 843k tokens");
        assert!(detail.contains("$50.55") && detail.ends_with("7h 48m"), "{detail}");
    }

    #[test]
    fn counts_down_near_the_limit() {
        let near = Some(Limit { used: 96.0, resets_at: Some(T + 2 * HOUR + 10 * 60) });
        let far = Some(Limit { used: 40.0, resets_at: Some(T + HOUR) });
        assert_eq!(countdowns([near, far], 95, T), [Some("2h 10m".to_string()), None]);
        assert_eq!(countdowns([near, far], 0, T), [None, None]);
        // Reset already: nothing to count down.
        assert_eq!(countdowns([near, None], 95, T + 3 * HOUR), [None, None]);
    }
}
