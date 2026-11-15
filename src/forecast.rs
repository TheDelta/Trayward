//! Weekly limit planning: how the week splits into days, how much is left
//! per day, and where the current pace leads.
//!
//! The usage API only reports the current percentage, so the app keeps its
//! own history of samples (see `store.rs`). Pure logic, unit-tested.

use serde::{Deserialize, Serialize};

const DAY: i64 = 86_400;
/// The weekly limit's window.
pub const WEEK: i64 = 7 * DAY;
/// Samples are kept this long.
const KEEP: i64 = 56 * DAY;
/// A new sample is stored at most this often unless the value changed.
const SAMPLE_EVERY: i64 = 15 * 60;
/// The pace is only estimated from at least this much observed time.
const MIN_SPAN: i64 = 6 * 3600;

/// One reading of the weekly limit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Unix seconds.
    pub at: i64,
    /// Percent used.
    pub used: f64,
    /// The window's reset time; samples of one window share it.
    pub resets_at: Option<i64>,
    /// The plan it was read on (none in readings of earlier versions): a
    /// percentage of one plan's limit says nothing about another's.
    #[serde(default)]
    pub plan: Option<String>,
}

/// A fall of the percentage within a window by more than this isn't use: the
/// limit itself changed (a bigger plan), and earlier readings no longer compare.
const DROP: f64 = 0.5;

/// Whether two readings were taken on the same plan, as far as they say.
fn same_plan(a: &Sample, b: &Sample) -> bool {
    match (&a.plan, &b.plan) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// Whether `b` (read after `a`) can be compared with `a`: the same window, the
/// same plan, and no fall in between.
fn comparable(a: &Sample, b: &Sample) -> bool {
    same_window(a.resets_at, b.resets_at) && same_plan(a, b) && b.used >= a.used - DROP
}

/// The readings of the window ending at `resets_at` that compare with the
/// newest one: back to its start, or to a change of plan or a fall of the
/// percentage. And whether such a change cut them off (then the window's
/// start says nothing about what was used: it was another limit).
fn segment(history: &[Sample], resets_at: i64) -> (&[Sample], bool) {
    let Some(end) = history.iter().rposition(|s| same_window(s.resets_at, Some(resets_at))).map(|i| i + 1) else {
        return (&[], false);
    };
    let mut start = end - 1;
    while start > 0 && same_window(history[start - 1].resets_at, Some(resets_at)) {
        if !comparable(&history[start - 1], &history[start]) {
            return (&history[start..end], true);
        }
        start -= 1;
    }
    (&history[start..end], false)
}

/// How a day's budget is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Budget {
    /// What's left of the week when a day begins, spread over it and the days
    /// after: what a day leaves unused goes to the days after it, what it uses
    /// beyond its budget comes off them.
    #[default]
    Adaptive,
    /// A seventh of the week, every day.
    Even,
}

/// Whether two reset times belong to the same weekly window. The API's
/// reset time wobbles by a second or so between requests.
fn same_window(a: Option<i64>, b: Option<i64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() < 3600,
        (a, b) => a == b,
    }
}

/// Adds a reading to the history, skipping near-duplicates and old samples.
pub fn record(history: &mut Vec<Sample>, s: Sample) {
    if let Some(last) = history.last() {
        let same = (last.used - s.used).abs() < 0.05 && same_window(last.resets_at, s.resets_at) && same_plan(last, &s);
        if same && s.at - last.at < SAMPLE_EVERY {
            return;
        }
    }
    let at = s.at;
    history.push(s);
    history.retain(|x| at - x.at <= KEEP);
}

/// Where the week stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// How much of the window has passed, 0 to 1: where an even spread would be now.
    pub elapsed: f64,
    /// Percent used now.
    pub used: f64,
    /// Which day of the window it is, 0 to 6.
    pub today: usize,
    /// Days until the reset (fractional).
    pub days_left: f64,
    /// What can still be used per day to just reach 100 % at the reset.
    pub per_day_left: f64,
    /// Used since today's day of the window began, if a sample from before it exists.
    pub today_used: Option<f64>,
    /// Average use per day the forecast goes by: this week's
    /// ([`Plan::this_week`]), or, early in the week, the past weeks'.
    pub pace: Option<f64>,
    /// `pace` is this week's own, from its first hours on: how the week goes
    /// now matters more than how earlier weeks went.
    pub this_week: bool,
    /// Average use per day in the past weeks (the readings Trayward keeps),
    /// to compare with.
    pub past_pace: Option<f64>,
    /// Expected percentage at the reset at that pace.
    pub at_reset: Option<f64>,
    /// When 100 % would be reached at that pace, if before the reset.
    pub full_at: Option<i64>,
    /// Each day of the window: what it may use and what it used.
    pub days: [Day; 7],
}

/// One day of the weekly window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Day {
    /// What the day may use (percent of the week): what was left when it
    /// began, spread evenly over it and the days after it. Days still to
    /// come share what's left now.
    pub budget: f64,
    /// What it used (percent of the week), for days that began, when the
    /// readings tell.
    pub used: Option<f64>,
}

impl Day {
    /// Used, as a share of the day's budget (100: all of it).
    pub fn share(&self) -> Option<f64> {
        self.used.filter(|_| self.budget > 0.0).map(|u| u / self.budget * 100.0)
    }

    /// It used more than its budget.
    pub fn over(&self) -> bool {
        self.used.is_some_and(|u| u > self.budget + 0.05)
    }
}

/// How the week stands against an even spread of the limit, from good to bad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pace {
    /// At or under an even pace.
    Under,
    /// Over it: slow down. Also from 80 % used.
    Over,
    /// This week's pace uses the limit up well before the reset (it would
    /// reach more than [`RUNS_OUT`] % by then). Also from 95 % used.
    Out,
}

/// How far over an even pace (percentage points) still counts as on it.
const ON_PACE: f64 = 1.0;
/// Where this week's pace would end at the reset (percent) when it counts as
/// running out: a little over 100, so being just over an even pace is a
/// warning, not an alarm.
pub const RUNS_OUT: f64 = 110.0;

impl Plan {
    /// Today, as [`Plan::days`] has it.
    pub fn today_day(&self) -> Day {
        self.days[self.today]
    }

    /// How far use is over (positive) or under an even spread of the limit, in
    /// percentage points.
    pub fn ahead(&self) -> f64 {
        self.used - self.elapsed * 100.0
    }

    /// How the week stands: over an even pace, or on course to use the limit
    /// up well before the reset (by this week's own pace, not the past
    /// weeks'), and at least as bad as the limit's own warnings at 80 and 95 % used.
    pub fn standing(&self) -> Pace {
        let runs_out = self.this_week && self.at_reset.is_some_and(|at| at > RUNS_OUT);
        let by_pace = match (self.ahead() > ON_PACE, runs_out) {
            (true, true) => Pace::Out,
            (true, false) => Pace::Over,
            (false, _) => Pace::Under,
        };
        let by_use = if self.used >= 95.0 {
            Pace::Out
        } else if self.used >= 80.0 {
            Pace::Over
        } else {
            Pace::Under
        };
        by_pace.max(by_use)
    }
}

/// Average percent per day over the history: the rise within each window,
/// divided by the time observed.
fn pace(history: &[Sample]) -> Option<f64> {
    let mut rise = 0.0;
    let mut span = 0;
    let mut i = 0;
    while i < history.len() {
        // A stretch of comparable readings: one window, one plan, no fall.
        let mut j = i;
        while j + 1 < history.len() && comparable(&history[j], &history[j + 1]) {
            j += 1;
        }
        rise += (history[j].used - history[i].used).max(0.0);
        span += history[j].at - history[i].at;
        i = j + 1;
    }
    (span >= MIN_SPAN).then(|| rise / (span as f64 / DAY as f64))
}

pub fn plan(history: &[Sample], used: f64, resets_at: i64, now: i64) -> Plan {
    plan_with(history, used, resets_at, now, Budget::Adaptive)
}

pub fn plan_with(history: &[Sample], used: f64, resets_at: i64, now: i64, budget: Budget) -> Plan {
    let start = resets_at - WEEK;
    let today = ((now - start).max(0) / DAY).min(6) as usize;
    let days_left = ((resets_at - now) as f64 / DAY as f64).max(1.0 / 24.0);
    let per_day_left = ((100.0 - used).max(0.0)) / days_left;
    // Only readings that compare with now: of this window, on this plan.
    let (readings, cut_off) = segment(history, resets_at);
    // This week's own pace: what it used since it began (or since the plan
    // changed), over the time since; once a few hours have passed.
    let week_pace = if cut_off {
        readings.first().map(|first| (used - first.used, now - first.at))
    } else {
        Some((used, now - start))
    }
    .filter(|(_, span)| *span >= MIN_SPAN)
    .map(|(rise, span)| rise.max(0.0) / (span as f64 / DAY as f64));
    let past: Vec<Sample> = history.iter().filter(|s| !same_window(s.resets_at, Some(resets_at))).cloned().collect();
    let past_pace = pace(&past);
    let this_week = week_pace.is_some();
    let pace = week_pace.or(past_pace);
    let at_reset = pace.map(|p| used + p * days_left);
    let full_at = pace
        .filter(|p| *p > 0.0 && used + p * days_left > 100.0)
        .map(|p| now + ((100.0 - used).max(0.0) / p * DAY as f64) as i64);
    let elapsed = ((now - start) as f64 / WEEK as f64).clamp(0.0, 1.0);
    // What was used when a day began: the last reading before it (0 at the
    // window's start, unless the plan changed since). Without one, the day's
    // first reading: what came before it that day isn't known.
    let began_with = |begin: i64| {
        if begin <= start && !cut_off {
            return Some(0.0);
        }
        readings
            .iter()
            .rev()
            .find(|s| s.at <= begin)
            .or_else(|| readings.iter().find(|s| s.at > begin && s.at < begin + DAY))
            .map(|s| s.used)
    };
    let even = 100.0 / 7.0;
    let days = std::array::from_fn(|i| {
        let begin = start + i as i64 * DAY;
        if i > today {
            let share = if budget == Budget::Even { even } else { per_day_left };
            return Day { budget: share, used: None };
        }
        let before = began_with(begin);
        let share = match (budget, before) {
            (Budget::Adaptive, Some(b)) => (100.0 - b).max(0.0) / (7 - i) as f64,
            _ => even,
        };
        // Today: until now; a day before: until the next began.
        let after = if i == today { Some(used) } else { began_with(begin + DAY) };
        let spent = before.zip(after).map(|(b, e)| (e - b).max(0.0));
        Day { budget: share, used: spent }
    });
    let today_used = days[today].used;
    Plan {
        elapsed,
        used,
        today,
        days_left,
        per_day_left,
        today_used,
        pace,
        this_week,
        past_pace,
        at_reset,
        full_at,
        days,
    }
}

/// Length of a simulated session window, like the 5-hour limit of other plans.
pub const FIVE_HOURS: i64 = 5 * 3600;
/// The simulated limit of one window: a day's share of the weekly limit.
pub const FIVE_HOUR_SHARE: f64 = 100.0 / 7.0;

/// A simulated 5-hour window, for plans without a 5-hour limit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub started: i64,
    pub ends: i64,
    /// Weekly percent used since the window started.
    pub used: f64,
}

/// The current simulated 5-hour window, if one is running: like the real
/// limit, a window starts with the first use after the previous one ended and
/// lasts five hours. `used` is the weekly percent now.
pub fn five_hour_window(history: &[Sample], used: f64, resets_at: Option<i64>, now: i64) -> Option<Window> {
    let mut window: Option<(i64, f64, Option<i64>)> = None; // start, weekly % before it, its weekly window
    let mut prev: Option<&Sample> = None;
    for s in history.iter().filter(|s| s.at <= now) {
        if let Some(p) = prev {
            let reset = !same_window(p.resets_at, s.resets_at);
            let rose = if reset { s.used > 0.0 } else { s.used > p.used };
            let open = window.is_some_and(|(start, _, _)| s.at < start + FIVE_HOURS);
            if rose && !open {
                // Use started between the two readings; the earlier one is the base.
                window = Some((p.at, if reset { 0.0 } else { p.used }, s.resets_at));
            }
        }
        prev = Some(s);
    }
    let (started, base, weekly) = window?;
    let ends = started + FIVE_HOURS;
    if now >= ends {
        return None;
    }
    let base = if same_window(weekly, resets_at) { base } else { 0.0 };
    Some(Window { started, ends, used: (used - base).max(0.0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESET: i64 = 1_000 * DAY;

    fn s(at: i64, used: f64) -> Sample {
        Sample { at, used, resets_at: Some(RESET), plan: None }
    }

    fn on(plan: &str, at: i64, used: f64) -> Sample {
        Sample { plan: Some(plan.into()), ..s(at, used) }
    }

    #[test]
    fn stands_by_pace_and_use() {
        let start = RESET - WEEK;
        // Half the week gone: 40 % is under an even pace, 60 % over it.
        let at = |used: f64| plan(&[s(start, 0.0), s(start + 3 * DAY, used * 0.9)], used, RESET, start + WEEK / 2);
        assert_eq!(at(40.0).standing(), Pace::Under);
        assert_eq!(at(50.5).standing(), Pace::Under);
        // 53 % at half the week: over, ending a little past 100 % at this pace.
        assert_eq!(at(53.0).standing(), Pace::Over);
        // 75 %: this week's pace uses it up well before the reset.
        assert_eq!(at(75.0).standing(), Pace::Out);
        // Late in the week, 85 % is a warning even on pace.
        let late = plan(&[s(start, 0.0)], 85.0, RESET, start + WEEK - DAY / 2);
        assert_eq!(late.standing(), Pace::Over);
    }

    #[test]
    fn a_change_of_plan_starts_over() {
        let start = RESET - WEEK;
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // Max: 10 % a day for two days, then an upgrade to Ultra on day 2: the
        // same use is a quarter of the bigger limit (30 % → 7.5 %).
        let up = [
            on("Max", start + 3600, 0.0),
            on("Max", start + DAY - 60, 10.0),
            on("Max", start + 2 * DAY - 60, 20.0),
            on("Max", start + 2 * DAY + 3 * 3600, 30.0),
            on("Ultra", start + 2 * DAY + 4 * 3600, 7.5),
            on("Ultra", start + 2 * DAY + 8 * 3600, 9.0),
        ];
        let now = start + 2 * DAY + 9 * 3600;
        let p = plan(&up, 9.5, RESET, now);
        // Today is counted from the first reading on Ultra; the days before say nothing about it.
        assert!(close(p.today_day().used.unwrap(), 2.0));
        assert_eq!((p.days[0].used, p.days[1].used), (None, None));
        // Its budget: what was left on Ultra, over the days left.
        assert!(close(p.today_day().budget, (100.0 - 7.5) / 5.0));
        // The fall from 30 % to 7.5 % isn't use, and the jump isn't either.
        let rate = pace(&up).unwrap();
        assert!(rate < 15.0, "{rate}");

        // A downgrade (Ultra → Max): the same use is four times as much.
        let down = [
            on("Ultra", start + 2 * DAY - 60, 5.0),
            on("Ultra", start + 2 * DAY + 3600, 6.0),
            on("Max", start + 2 * DAY + 2 * 3600, 24.0),
            on("Max", start + 2 * DAY + 5 * 3600, 26.0),
        ];
        let p = plan(&down, 26.0, RESET, start + 2 * DAY + 6 * 3600);
        // The jump to 24 % isn't today's use: today counts from the first reading on Max.
        assert!(close(p.today_day().used.unwrap(), 2.0));
        assert!(pace(&down).is_none_or(|p| p < 15.0));

        // Without the plan's name (it's asked for only now and then), a fall still tells.
        let unnamed =
            [s(start + 2 * DAY - 60, 30.0), s(start + 2 * DAY + 3600, 8.0), s(start + 2 * DAY + 4 * 3600, 9.0)];
        let p = plan(&unnamed, 9.0, RESET, start + 2 * DAY + 5 * 3600);
        assert!(close(p.today_day().used.unwrap(), 1.0));
    }

    #[test]
    fn an_even_budget_is_a_seventh_every_day() {
        let start = RESET - WEEK;
        let h = [s(start + 3600, 2.0), s(start + DAY - 60, 30.0)];
        let now = start + DAY + 3600;
        let p = plan_with(&h, 31.0, RESET, now, Budget::Even);
        assert!(p.days.iter().all(|d| (d.budget - 100.0 / 7.0).abs() < 1e-9));
        // Day 0 used 30 %: over its seventh; adaptive, day 1 gets less than a seventh.
        assert!(p.days[0].over());
        let adaptive = plan(&h, 31.0, RESET, now);
        assert!((adaptive.days[1].budget - 70.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn a_day_without_a_reading_before_it_counts_from_its_first() {
        let start = RESET - WEEK;
        // Trayward started on day 3 at 10:00 (nothing before): today counts from then.
        let h = [s(start + 3 * DAY + 10 * 3600, 40.0), s(start + 3 * DAY + 12 * 3600, 43.0)];
        let p = plan(&h, 44.0, RESET, start + 3 * DAY + 13 * 3600);
        assert!((p.today_day().used.unwrap() - 4.0).abs() < 1e-9);
        assert!((p.today_day().budget - 60.0 / 4.0).abs() < 1e-9);
        // The days before have no reading at all: unknown, an even share.
        assert_eq!(p.days[1].used, None);
    }

    #[test]
    fn records_without_duplicates() {
        let mut h = Vec::new();
        record(&mut h, s(0, 1.0));
        record(&mut h, s(60, 1.0)); // same value, too soon
        record(&mut h, s(120, 2.0)); // changed
        record(&mut h, s(120 + SAMPLE_EVERY, 2.0)); // same value, but a while later
        assert_eq!(h.len(), 3);
        record(&mut h, s(KEEP + 200, 3.0));
        assert_eq!(h.first().unwrap().at, 120 + SAMPLE_EVERY);
    }

    #[test]
    fn gives_each_day_a_budget() {
        let start = RESET - WEEK;
        // Day 0 used 10 %, day 1 used 20 % (over its share), now day 2 with 5 % so far.
        let h = [s(start + 3600, 2.0), s(start + DAY - 60, 10.0), s(start + 2 * DAY - 60, 30.0)];
        let now = start + 2 * DAY + 6 * 3600;
        let p = plan(&h, 35.0, RESET, now);
        assert_eq!(p.today, 2);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // Day 0: the even share; used 10.
        assert!(close(p.days[0].budget, 100.0 / 7.0) && close(p.days[0].used.unwrap(), 10.0));
        // Day 1: 90 left over 6 days = 15; used 20: over.
        assert!(close(p.days[1].budget, 15.0) && close(p.days[1].used.unwrap(), 20.0));
        assert!(p.days[1].over() && !p.days[0].over());
        // Today: 70 left over 5 days = 14; used 5 so far: about a third.
        assert!(close(p.today_day().budget, 14.0) && close(p.today_day().used.unwrap(), 5.0));
        assert!(close(p.today_day().share().unwrap(), 5.0 / 14.0 * 100.0));
        // Days to come share what's left now; they used nothing yet.
        assert!(close(p.days[3].budget, p.per_day_left) && p.days[3].used.is_none());
        // Without readings, a past day's use is unknown and its budget the even share.
        let p = plan(&[], 35.0, RESET, now);
        assert_eq!((p.days[1].used, p.today_day().used), (None, None));
        assert!(close(p.days[1].budget, 100.0 / 7.0));
        assert!(p.today_day().share().is_none() && !p.today_day().over());
    }

    #[test]
    fn splits_the_week_into_days() {
        let now = RESET - 2 * DAY - 3600; // day 4 of 0..6
        let p = plan(&[], 40.0, RESET, now);
        assert_eq!(p.today, 4);
        assert!((p.days_left - (2.0 + 1.0 / 24.0)).abs() < 1e-9);
        assert!((p.per_day_left - 60.0 / p.days_left).abs() < 1e-9);
        assert_eq!(p.today_used, None);
        // Day 4 with 40 % used: this week's pace, from the week's start.
        assert!(p.this_week);
        assert!((p.pace.unwrap() - 40.0 / (5.0 - 1.0 / 24.0)).abs() < 1e-9);
        assert_eq!(p.past_pace, None);
    }

    #[test]
    fn todays_use_and_pace() {
        let start = RESET - WEEK;
        // 10 % per day so far.
        let history = [s(start + 3600, 0.0), s(start + DAY, 10.0), s(start + 2 * DAY, 20.0)];
        let now = start + 2 * DAY + 6 * 3600;
        let p = plan(&history, 23.0, RESET, now);
        assert_eq!(p.today, 2);
        assert!((p.today_used.unwrap() - 3.0).abs() < 1e-9);
        // This week: 23 % in two and a quarter days.
        assert!((p.pace.unwrap() - 23.0 / 2.25).abs() < 1e-9);
        assert!(p.at_reset.unwrap() < 100.0);
        assert_eq!(p.full_at, None);
    }

    #[test]
    fn the_forecast_goes_by_this_week_not_the_past() {
        let start = RESET - WEEK;
        let last = RESET - WEEK;
        // Last week: 50 % a day.
        let past: Vec<Sample> = (0..=2)
            .map(|d| Sample { at: start - WEEK + d * DAY, used: 50.0 * d as f64, resets_at: Some(last), plan: None })
            .collect();
        // This week: 4 % a day so far.
        let mut h = past.clone();
        h.push(s(start + DAY, 4.0));
        h.push(s(start + 2 * DAY, 8.0));
        let now = start + 2 * DAY;
        let p = plan(&h, 8.0, RESET, now);
        assert!(p.this_week);
        assert!((p.pace.unwrap() - 4.0).abs() < 1e-9);
        assert!((p.past_pace.unwrap() - 50.0).abs() < 1e-9);
        // So no warning: about 28 % at the reset.
        assert_eq!(p.full_at, None);
        assert!((p.at_reset.unwrap() - 28.0).abs() < 1e-9);
        // In the first hours of the week, the past weeks' pace is all there is.
        let early = plan(&past, 0.5, RESET, start + 3600);
        assert!(!early.this_week);
        assert!((early.pace.unwrap() - 50.0).abs() < 1e-9);
        // After a change of plan, this week counts from the first reading on the new one.
        let mut changed = h.clone();
        changed.push(on("Ultra", start + 2 * DAY + 60, 2.0));
        changed.push(on("Ultra", start + 2 * DAY + 12 * 3600, 3.0));
        let p = plan(&changed, 3.0, RESET, start + 2 * DAY + 12 * 3600 + 60);
        assert!(p.this_week && p.pace.unwrap() < 3.0);
    }

    #[test]
    fn warns_when_the_pace_runs_out_early() {
        let start = RESET - WEEK;
        let history = [s(start, 0.0), s(start + DAY, 40.0)];
        let now = start + DAY;
        let p = plan(&history, 40.0, RESET, now);
        let full = p.full_at.unwrap();
        assert!((full - (now + (60.0 / 40.0 * DAY as f64) as i64)).abs() <= 1);
        assert!(full < RESET);
    }

    #[test]
    fn knows_how_much_of_the_week_passed() {
        let p = plan(&[], 0.0, RESET, RESET - WEEK / 4);
        assert!((p.elapsed - 0.75).abs() < 1e-9);
    }

    #[test]
    fn simulates_five_hour_windows() {
        let h = [s(0, 10.0), s(3600, 10.0), s(4000, 12.0), s(8000, 13.0)];
        // Use began between 3600 and 4000: the window starts at 3600 from 10 %.
        let w = five_hour_window(&h, 14.0, Some(RESET), 9000).unwrap();
        assert_eq!((w.started, w.ends), (3600, 3600 + FIVE_HOURS));
        assert!((w.used - 4.0).abs() < 1e-9);
        // Over after five hours.
        assert_eq!(five_hour_window(&h, 14.0, Some(RESET), 3600 + FIVE_HOURS), None);
        // A later rise starts a new window.
        let mut later = h.to_vec();
        later.push(s(3600 + FIVE_HOURS + 100, 13.0));
        later.push(s(3600 + FIVE_HOURS + 200, 15.0));
        let w = five_hour_window(&later, 15.0, Some(RESET), 3600 + FIVE_HOURS + 300).unwrap();
        assert_eq!(w.started, 3600 + FIVE_HOURS + 100);
        assert!((w.used - 2.0).abs() < 1e-9);
        // No use, no window.
        assert_eq!(five_hour_window(&[s(0, 5.0), s(100, 5.0)], 5.0, Some(RESET), 200), None);
    }

    #[test]
    fn reset_time_wobble_is_the_same_window() {
        let a = s(0, 4.0);
        let b = Sample { at: 3600, used: 5.0, resets_at: Some(RESET - 1), plan: None };
        let w = five_hour_window(&[a, b], 5.0, Some(RESET + 1), 4000).unwrap();
        assert!((w.used - 1.0).abs() < 1e-9);
    }

    #[test]
    fn pace_ignores_resets() {
        // A reset drops the value; only rises within a window count.
        let old = Sample { at: 0, used: 90.0, resets_at: Some(RESET - WEEK), plan: None };
        let history = [old.clone(), Sample { at: DAY / 2, used: 95.0, ..old }, s(DAY / 2 + 60, 0.0), s(DAY, 5.0)];
        let p = pace(&history).unwrap();
        assert!((p - 10.0 / ((DAY / 2 + DAY / 2 - 60) as f64 / DAY as f64)).abs() < 1e-6);
    }
}
