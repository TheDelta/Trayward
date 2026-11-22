//! The flyout that opens above the widget: usage details and the session list.
//! Clicking a session switches to it; the × on a row hides it. Long lists
//! expand on request and scroll when the flyout would not fit on the screen.

use crate::forecast;
use crate::forge;
use crate::gfx::{self, Canvas, NORMAL, SEMIBOLD};
use crate::i18n::{f, t};
use crate::sessions::Status;
use crate::theme::{self, Palette};
use crate::timefmt;
use crate::tracking;
use crate::usage::Limit;
use std::path::PathBuf;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    PAINTSTRUCT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, HWND_TOPMOST, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetForegroundWindow,
    SetWindowPos, ShowWindow, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, Result, w};

pub const CLASS: PCWSTR = w!("Trayward.Flyout");

const WIDTH: f32 = 420.0;
/// Claude's usage page, which the flyout's title opens.
const USAGE_PAGE: &str = "https://claude.ai/settings/usage";
const PAD: f32 = 16.0;
const ROW_H: f32 = 48.0;
/// Extra height of a row with background work.
const WATCH_H: f32 = 26.0;
const RECENT_ROW_H: f32 = 30.0;
/// Most session rows shown in total, and most of them for active sessions,
/// until the list is expanded.
const MAX_ROWS: usize = 12;
const MAX_ACTIVE_ROWS: usize = 8;
/// A scrolling list shows at least this much.
const MIN_LIST_H: f32 = 2.0 * ROW_H;
/// One mouse-wheel notch scrolls a row.
const WHEEL_STEP: f32 = ROW_H;
/// Archived sessions are drawn faded.
const ARCHIVED_OPACITY: f32 = 0.55;
/// Gap between the flyout and the taskbar / screen edge.
const MARGIN: f32 = 12.0;

#[derive(Clone, Debug, PartialEq)]
pub struct SessionRow {
    pub id: String,
    /// Name of the VS Code window (or the folder).
    pub workspace: String,
    /// Conversation title.
    pub title: String,
    pub status: Status,
    /// A finished session that was already looked at (shown quieter).
    pub seen: bool,
    pub status_text: String,
    /// Model, tokens and agents, e.g. "Opus 5.5 · 142k context · 2 agents".
    pub details: String,
    /// Runs in VS Code (shows its icon).
    pub vscode: bool,
    /// Archived in the Claude Code extension (shows a tag).
    pub archived: bool,
    /// A session of another provider, by its name, e.g. "Codex" (shows it as
    /// a tag; no statistics, can't be hidden).
    pub other: Option<&'static str>,
    /// Its branch's merge or pull request, or pipeline (see [`ChangeLine`]).
    pub change: Option<ChangeLine>,
    /// Pinned to the taskbar.
    pub pinned: bool,
    /// Seconds until the prompt cache expires (negative: expired), if known.
    pub cache_left: Option<i64>,
    /// Has needed the user for a long time.
    pub urgent: bool,
    /// Background work still running, e.g. "Wait for the pipeline".
    pub watching: Option<String>,
    /// Subagents running now (shown as a badge before the state).
    pub agents: usize,
    /// The context is getting full (see `transcripts::context_fill`).
    pub context: Option<crate::transcripts::Fill>,
}

/// "1 agent", "3 agents".
pub fn agents_label(n: usize) -> String {
    if n == 1 { t("1 agent").to_string() } else { f("{0} agents", &[&n]) }
}

/// A session's merge or pull request (or its branch's pipeline), as a pill
/// under it: e.g. "!37 · Pipeline running"; a click opens it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeLine {
    pub text: String,
    pub url: Option<String>,
    pub checks: Option<forge::Checks>,
    pub merged: bool,
}

/// The words for a pipeline's (or GitHub's checks') state.
pub fn checks_text(f: forge::Forge, c: forge::Checks) -> &'static str {
    use forge::{Checks, Forge};
    match (f, c) {
        (Forge::GitLab, Checks::Running) => t("Pipeline running"),
        (Forge::GitLab, Checks::Passed) => t("Pipeline passed"),
        (Forge::GitLab, Checks::Failed) => t("Pipeline failed"),
        (Forge::GitLab, Checks::Canceled) => t("Pipeline canceled"),
        (Forge::GitHub, Checks::Running) => t("Checks running"),
        (Forge::GitHub, Checks::Passed) => t("Checks passed"),
        (Forge::GitHub, Checks::Failed) => t("Checks failed"),
        (Forge::GitHub, Checks::Canceled) => t("Checks canceled"),
    }
}

/// The colour for a pipeline's state, or a merged request.
fn checks_color(pal: &Palette, checks: Option<forge::Checks>, merged: bool) -> crate::theme::Color {
    use forge::Checks;
    match (merged, checks) {
        (true, _) => pal.plan_to,
        (_, Some(Checks::Running)) => pal.working,
        (_, Some(Checks::Passed)) => pal.done,
        (_, Some(Checks::Failed)) => pal.crit,
        _ => pal.text_dim,
    }
}

/// Codex's limits, as of its last use.
#[derive(Clone, Debug, PartialEq)]
pub struct Codex {
    /// e.g. "Plus".
    pub plan: Option<String>,
    pub weekly: Option<Limit>,
    pub five_hour: Option<Limit>,
    /// When Codex reported them (unix seconds).
    pub at: i64,
}

/// Copilot's monthly limit.
#[derive(Clone, Debug, PartialEq)]
pub struct Copilot {
    /// e.g. "Free".
    pub plan: Option<String>,
    /// What the limit counts, with how many are left: "Premium requests · 225 of 300 left".
    pub label: String,
    pub limit: Limit,
    /// When GitHub was asked (unix seconds).
    pub at: i64,
}

/// The GPG key timer turns amber in its last minutes.
const GPG_ENDING_SECS: i64 = 5 * 60;

/// Below this many seconds left the cache timer turns red and glows.
pub const CACHE_WARN_SECS: i64 = 10 * 60;

/// The GPG signing key: unlocked or not, and roughly how long it stays so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gpg {
    pub unlocked: bool,
    /// Seconds left, when it can be estimated.
    pub left: Option<i64>,
    /// The full time one signature keeps it unlocked.
    pub total: i64,
    /// `left` is a lower bound: when it was unlocked isn't known.
    pub at_least: bool,
}

/// State of the refresh button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    Ready,
    /// Spinning; the angle in degrees.
    Busy(u16),
    /// Just refreshed; ignores clicks for a moment.
    Resting,
}

impl Refresh {
    pub fn is_busy(self) -> bool {
        matches!(self, Refresh::Busy(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub dark: bool,
    pub now: i64,
    pub weekly: Option<Limit>,
    pub five_hour: Option<Limit>,
    /// Fable's own limit within the weekly one, when the plan has one and it's shown.
    pub fable: Option<crate::usage::ModelLimit>,
    /// Claude Code is used on this PC: without it, its limits are left out.
    pub claude: bool,
    pub plan: Option<String>,
    /// The week split into days, the budget left and the pace, if known.
    pub weekly_plan: Option<forecast::Plan>,
    /// How the weekly bar shows the days.
    pub week_bar: WeekBar,
    /// `five_hour` is simulated (the plan has no 5-hour limit).
    pub five_hour_simulated: bool,
    pub updated_at: Option<i64>,
    pub error: Option<String>,
    pub tracking: tracking::State,
    /// Visible sessions in display order (active first, then idle).
    pub sessions: Vec<SessionRow>,
    /// How many sessions the user hid.
    pub hidden: usize,
    pub refresh: Refresh,
    /// VS Code's exe, for its icon.
    pub vscode_exe: Option<PathBuf>,
    /// Glow strength (0 to 1) of things that pulse, like an expiring cache.
    pub pulse: f32,
    /// Where the shine on the Ultra plan's name is (see `plan::shine`).
    pub shine: Option<f32>,
    /// Codex's limits, when Codex is on and has used them.
    pub codex: Option<Codex>,
    /// Copilot's monthly limit, when it's on and known.
    pub copilot: Option<Copilot>,
    /// The user's own open merge and pull requests (when switched on).
    pub mine: Vec<forge::Change>,
    /// The session mailbox, when switched on (`mailbox.rs`).
    pub mailbox: Option<crate::mailbox::Snapshot>,
    /// The user's own extensions that are switched on and have flyout rows.
    pub extensions: Vec<crate::extensions::Extension>,
    /// The GPG key for signing commits, if shown.
    pub gpg: Option<Gpg>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Session(String),
    Hide(String),
    UnhideAll,
    Refresh,
    Settings,
    /// Statistics: of one session, or the overview.
    Stats(Option<String>),
    /// Open a web page (a merge or pull request).
    Open(String),
    /// Pin a session to the taskbar, or unpin it.
    Pin(String),
    EnableTracking,
    /// Show all sessions, or only the first few again.
    MoreSessions,
    FewerSessions,
    /// A day of the weekly bar (0 to 6): pointing at it shows its numbers; no click.
    Day(usize),
    /// Open a folder in Explorer (a mailbox inbox).
    Folder(std::path::PathBuf),
}

/// How the session list is shown: the flyout's own state, not the app's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct View {
    /// All sessions, not only the first few.
    pub expanded: bool,
    /// How far the list is scrolled (DIPs).
    pub scroll: f32,
    /// The list's visible height when it scrolls; `None`: it shows in full.
    pub list_max: Option<f32>,
}

/// What a layout pass returns.
pub struct Layout {
    pub height: f32,
    pub hits: Vec<Hit>,
    /// Height of the whole session list, scrolled or not.
    pub list_h: f32,
}

pub type Hit = crate::hits::Hit<Action, usize>;

/// A tooltip: where it points (x, y: below what it's about; `above`: that
/// thing's top, for when there's no room below), its title and its lines
/// (`true`: a warning).
struct Tip {
    x: f32,
    y: f32,
    above: f32,
    title: String,
    /// After the title, dim and not bold: when a day runs.
    note: String,
    lines: Vec<(String, Ink)>,
}

/// The colour of a tooltip's line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ink {
    Dim,
    Good,
    Warn,
    Bad,
}

/// Collects clickable areas while laying out and answers hover questions.
struct Hits<'a> {
    list: Vec<Hit>,
    /// Hovered hit (index, group), from the previous layout of the same model.
    hover: Option<(usize, usize)>,
    canvas: Option<&'a Canvas<'a>>,
    /// A tooltip to draw over everything once the rest is drawn: where it
    /// points (x, y), its title and lines (`true`: a warning).
    tip: Option<Tip>,
}

impl Hits<'_> {
    /// Whether the hovered hit belongs to `group`.
    fn group_hovered(&self, group: usize) -> bool {
        self.hover.is_some_and(|(_, g)| g == group)
    }
    /// Whether the hit about to be pushed (index `len`) is hovered itself.
    fn next_hovered(&self) -> bool {
        self.hover.is_some_and(|(i, _)| i == self.list.len())
    }
    fn push(&mut self, x: f32, y: f32, w: f32, h: f32, group: usize, action: Action) {
        self.list.push(Hit { x, y, w, h, action, tag: group });
    }
}

/// A text link with a hover background; returns its height.
fn link(g: &gfx::Gfx, hits: &mut Hits, pal: &Palette, label: &str, y: f32, action: Action) -> Result<f32> {
    let t = g.text(label, 13.0, SEMIBOLD, None)?;
    let (x, w, h) = (PAD - 6.0, t.width + 12.0, t.height + 8.0);
    let group = hits.list.len() + 1000;
    if let Some(c) = hits.canvas {
        if hits.next_hovered() {
            c.fill_round_rect(x, y, w, h, 4.0, pal.row_hover);
        }
        c.text(&t, PAD, y + 4.0, pal.accent);
    }
    hits.push(x, y, w, h, group, action);
    Ok(h)
}

/// Lays out the flyout and returns its height and clickable areas.
/// With a canvas it also draws, highlighting `hover` (hit index and group).
pub fn render<'a>(c: Option<&'a Canvas<'a>>, m: &Model, hover: Option<(usize, usize)>, v: &View) -> Result<Layout> {
    let g = gfx::get()?;
    let pal: &Palette = theme::palette(m.dark);
    let inner = WIDTH - 2.0 * PAD;
    let mut hits = Hits { list: Vec::new(), hover, canvas: c, tip: None };
    let mut y = PAD;
    let icon = m.vscode_exe.as_deref().and_then(|exe| g.exe_icon(exe));

    // Header: title, plan, status of the data, refresh button.
    let title = g.text(t("Usage"), 14.0, SEMIBOLD, None)?;
    let plan =
        m.plan.as_deref().map(|p| g.text(&crate::plan::label(p), 13.0, SEMIBOLD, None).map(|t| (t, p))).transpose()?;
    let refresh_hovered = hits.next_hovered();
    let note = match (m.refresh, refresh_hovered) {
        (Refresh::Busy(_), _) => t("Updating usage and sessions…").to_string(),
        (Refresh::Resting, true) => t("Just updated").to_string(),
        (Refresh::Ready, true) => t("Update usage and sessions now").to_string(),
        _ => match m.updated_at {
            Some(at) => f("Updated {0}", &[&timefmt::local_time(at, false)]),
            None => t("Not updated yet").into(),
        },
    };
    let note = g.text(&note, 12.0, NORMAL, None)?;
    let glyph = g.text("↻", 15.0, NORMAL, None)?;
    let gear = g.glyph('\u{E713}', 14.0)?;
    let (rw, rh) = (glyph.width + 12.0, title.height + 6.0);
    let gear_w = gear.width + 12.0;
    let gx = WIDTH - PAD - gear_w + 6.0;
    let chart = g.glyph('\u{E9D2}', 14.0)?;
    let chart_w = chart.width + 12.0;
    let sx = gx - chart_w;
    let rx = sx - rw;
    if let Some(c) = c {
        c.text(&note, rx - 4.0 - note.width, y + (title.height - note.height) / 2.0, pal.text_dim);
        if refresh_hovered && m.refresh == Refresh::Ready {
            c.fill_round_rect(rx, y - 3.0, rw, rh, 4.0, pal.row_hover);
        }
        let gy = y + (title.height - glyph.height) / 2.0;
        match m.refresh {
            Refresh::Busy(angle) => c.text_rotated(&glyph, rx + 6.0, gy, pal.accent, f32::from(angle)),
            Refresh::Resting => c.text(&glyph, rx + 6.0, gy, pal.text_dim.with_alpha(0.45)),
            Refresh::Ready => c.text(&glyph, rx + 6.0, gy, pal.text_dim),
        }
    }
    hits.push(rx, y - 3.0, rw, rh, 0, Action::Refresh);
    if let Some(c) = c {
        if hits.next_hovered() {
            c.fill_round_rect(sx, y - 3.0, chart_w, rh, 4.0, pal.row_hover);
        }
        c.text(&chart, sx + 6.0, y + (title.height - chart.height) / 2.0, pal.text_dim);
    }
    hits.push(sx, y - 3.0, chart_w, rh, 2001, Action::Stats(None));
    if let Some(c) = c {
        if hits.next_hovered() {
            c.fill_round_rect(gx, y - 3.0, gear_w, rh, 4.0, pal.row_hover);
        }
        c.text(&gear, gx + 6.0, y + (title.height - gear.height) / 2.0, pal.text_dim);
    }
    hits.push(gx, y - 3.0, gear_w, rh, 2000, Action::Settings);
    // The title opens the usage page on claude.ai (it also has what Trayward
    // can't read, such as limit resets).
    let (tx, tw) = (PAD - 6.0, title.width + 12.0);
    let title_hovered = hits.next_hovered();
    if title_hovered {
        hits.tip = Some(Tip {
            x: tx + tw / 2.0,
            y: y + rh,
            above: y - 3.0,
            title: t("Opens your usage on claude.ai").to_string(),
            note: String::new(),
            lines: Vec::new(),
        });
    }
    if let Some(c) = c {
        if title_hovered {
            c.fill_round_rect(tx, y - 3.0, tw, rh, 4.0, pal.row_hover);
        }
        c.text(&title, PAD, y, pal.text);
        if let Some((p, name)) = &plan {
            let px = PAD + title.width + 8.0 + crate::plan::margin(name);
            crate::plan::draw(c, p, px, y + (title.height - p.height) / 2.0, name, pal, 1.0, m.shine);
        }
    }
    hits.push(tx, y - 3.0, tw, rh, 1999, Action::Open(USAGE_PAGE.to_string()));
    y += title.height + 10.0;

    // Limits. Plans without a 5-hour limit (e.g. Ultra) show only the weekly
    // one, unless a 5-hour limit is simulated.
    let no_five_hour = m.five_hour.is_none() && m.updated_at.is_some();
    let five_label = if m.five_hour_simulated { "5-hour (simulated)" } else { "5-hour" };
    // Fable's limit under the weekly one, with how much of the week it may use.
    let mut limits: Vec<(&str, Option<Limit>, Option<String>)> = vec![("Weekly", m.weekly, None)];
    if let Some(fable) = m.fable {
        let share = fable.share.map(|s| f("Up to {0}% of your weekly limit", &[&dec(s)]));
        limits.push(("Fable", Some(fable.limit), share));
    }
    if !no_five_hour {
        limits.push((five_label, m.five_hour, None));
    }
    if !m.claude {
        limits.clear();
    }
    for (label, limit, note) in limits {
        let used = limit.map(|l| l.used_at(m.now));
        let simulated = m.five_hour_simulated && label == five_label;
        let is_weekly = label == "Weekly" && m.weekly.is_some();
        let label = g.text(t(label), 13.0, NORMAL, None)?;
        let value = g.text(&used.map_or("–".into(), |u| format!("{}%", u.round())), 13.0, SEMIBOLD, None)?;
        let reset = match limit.and_then(|l| l.resets_at) {
            Some(t) if t > m.now => f(
                "Resets in {0} · {1}",
                &[&timefmt::countdown(t - m.now), &timefmt::local_time(t, t - m.now > 20 * 3600)],
            ),
            Some(_) => t("Reset, nothing used since").into(),
            None if simulated => t("Starts with your next use · a day's share of the week").into(),
            None if limit.is_some() => t("No reset scheduled").into(),
            None => t("No data yet").into(),
        };
        let reset = match note {
            Some(n) => format!("{n} · {reset}"),
            None => reset,
        };
        let reset = g.text(&reset, 12.0, NORMAL, Some(inner))?;
        let level = used.unwrap_or(0.0);
        let week = (is_weekly, m.weekly_plan, limit.and_then(|l| l.resets_at));
        let weekly_plan = match week {
            (true, Some(plan), Some(_)) => Some(plan),
            _ => None,
        };
        // The weekly limit in the colour of its pace; the others from 80 % used.
        let value_color = match weekly_plan {
            Some(plan) => pace_color(pal, plan.standing()),
            None if level >= 80.0 => pal.level(level),
            None => pal.text,
        };
        // The weekly limit: with one decimal, next to where an even pace would be
        // now ("14.5% of 15% even pace"), and today's use next to its name.
        let value = match (weekly_plan, used) {
            (Some(_), Some(u)) => g.text(&format!("{}%", dec(u)), 13.0, SEMIBOLD, None)?,
            _ => value,
        };
        let even = weekly_plan
            .map(|p| g.text(&f("of {0}% even pace", &[&dec(p.elapsed * 100.0)]), 12.0, NORMAL, None))
            .transpose()?;
        let today = match weekly_plan.and_then(|p| p.today_used.map(|u| (p, u))) {
            Some((p, u)) => Some((g.text(&f("(Today: {0})", &[&pct(u)]), 12.0, NORMAL, None)?, today_color(pal, p))),
            None => None,
        };
        if let Some(c) = c {
            c.text(&label, PAD, y, pal.text);
            if let Some((t, color)) = &today {
                c.text(t, PAD + label.width + 6.0, y + (label.height - t.height) / 2.0, *color);
            }
            let mut right = WIDTH - PAD;
            if let Some(e) = &even {
                right -= e.width;
                c.text(e, right, y + (label.height - e.height) / 2.0, pal.text_dim);
                right -= 5.0;
            }
            c.text(&value, right - value.width, y, value_color);
        }
        let bar_y = y + label.height + 5.0;
        if let Some(plan) = weekly_plan {
            y = week_bar(&g, &mut hits, pal, &plan, level, m.week_bar, bar_y)?;
            // On plans without a 5-hour limit, say so at the end of this line.
            let note =
                no_five_hour.then(|| g.text(t("No 5-hour limit on this plan"), 12.0, NORMAL, None)).transpose()?;
            if let Some(c) = c {
                c.text(&reset, PAD, y, pal.text_dim);
                if let Some(n) = &note {
                    c.text(n, WIDTH - PAD - n.width, y, pal.text_dim);
                }
            }
            y += reset.height + 2.0;
            let (line, tone) = prediction(&plan);
            let t = g.text(&line, 12.0, NORMAL, Some(inner))?;
            if let Some(c) = c {
                c.text(&t, PAD, y, tone.map_or(pal.text_dim, |p| pace_color(pal, p)));
            }
            y += t.height + 2.0 + 10.0;
        } else {
            if let Some(c) = c {
                c.fill_round_rect(PAD, bar_y, inner, 6.0, 3.0, pal.track);
                if level > 0.0 {
                    let fill = (inner * (level as f32) / 100.0).max(6.0);
                    c.fill_round_rect(PAD, bar_y, fill, 6.0, 3.0, pal.level(level));
                }
                c.text(&reset, PAD, bar_y + 10.0, pal.text_dim);
            }
            y += label.height + 5.0 + 6.0 + 4.0 + reset.height + 12.0;
        }
    }
    if let Some(cx) = &m.codex {
        let plan = cx.plan.as_deref().map(|p| format!("ChatGPT {p}"));
        let rows = [(t("Weekly").to_string(), cx.weekly), (t("5-hour").to_string(), cx.five_hour)];
        y = other_limits(
            &g,
            c,
            pal,
            (crate::widget::Provider::Codex, "Codex"),
            plan.as_deref(),
            cx.at,
            &rows,
            m.now,
            y,
        )?;
    }
    if let Some(cp) = &m.copilot {
        let rows = [(cp.label.clone(), Some(cp.limit))];
        y = other_limits(
            &g,
            c,
            pal,
            (crate::widget::Provider::Copilot, "Copilot"),
            cp.plan.as_deref(),
            cp.at,
            &rows,
            m.now,
            y,
        )?;
    }
    if let Some(err) = &m.error {
        let t = g.text(err, 12.0, NORMAL, Some(inner))?;
        if let Some(c) = c {
            c.text(&t, PAD, y - 4.0, pal.warn);
        }
        y += t.height + 8.0;
    }

    // Sessions.
    if let Some(c) = c {
        c.fill_rect(PAD, y, inner, 1.0, pal.divider);
    }
    y += 1.0 + 14.0;
    let header = g.text(t("Sessions"), 14.0, SEMIBOLD, None)?;
    let summary = g.text(&summary(m), 12.0, NORMAL, None)?;
    if let Some(c) = c {
        c.text(&header, PAD, y, pal.text);
        c.text(&summary, WIDTH - PAD - summary.width, y + (header.height - summary.height) / 2.0, pal.text_dim);
    }
    y += header.height + 6.0;

    if m.tracking == tracking::State::Outdated {
        y += link(&g, &mut hits, pal, t("Repair session hooks"), y, Action::EnableTracking)? + 4.0;
    }
    let (active, idle): (Vec<&SessionRow>, Vec<&SessionRow>) =
        m.sessions.iter().partition(|r| r.status != Status::Idle);
    if m.sessions.is_empty() {
        let t = g.text(t("No agent sessions."), 12.0, NORMAL, Some(inner))?;
        if let Some(c) = c {
            c.text(&t, PAD, y, pal.text_dim);
        }
        y += t.height + 6.0;
    }
    // Collapsed, the first few rows; expanded, all of them.
    let collapsed_active = active.len().min(MAX_ACTIVE_ROWS);
    let collapsed_idle = idle.len().min(MAX_ROWS.saturating_sub(collapsed_active));
    let cut = m.sessions.len() - collapsed_active - collapsed_idle;
    let (active_shown, idle_shown) =
        if v.expanded { (active.len(), idle.len()) } else { (collapsed_active, collapsed_idle) };
    // When the flyout wouldn't fit on the screen, the rows scroll inside `list_max`.
    let list_top = y;
    let scroll = if v.list_max.is_some() { v.scroll } else { 0.0 };
    let first_hit = hits.list.len();
    if let (Some(c), Some(h)) = (c, v.list_max) {
        c.push_clip(0.0, list_top, WIDTH, h);
    }
    y -= scroll;
    for (i, row) in active.iter().take(active_shown).enumerate() {
        y += session_row(&g, &mut hits, pal, row, icon.as_ref(), m.pulse, y, i + 1, false)?;
    }
    if idle_shown > 0 {
        let t = g.text(t("Recent"), 12.0, SEMIBOLD, None)?;
        y += if active_shown > 0 { 8.0 } else { 0.0 };
        if let Some(c) = c {
            c.text(&t, PAD, y, pal.text_dim);
        }
        y += t.height + 2.0;
        for (i, row) in idle.iter().take(idle_shown).enumerate() {
            y += session_row(&g, &mut hits, pal, row, icon.as_ref(), m.pulse, y, 100 + i, true)?;
        }
    }
    let list_h = y - (list_top - scroll);
    y = list_top + v.list_max.map_or(list_h, |h| h.min(list_h));
    if let Some(h) = v.list_max {
        // Rows scrolled out of view can't be clicked; their hits shrink to
        // nothing (instead of going away, so hover indices stay the same).
        for hit in hits.list.iter_mut().skip(first_hit) {
            let (top, bottom) = (hit.y.max(list_top), (hit.y + hit.h).min(list_top + h));
            hit.y = top;
            hit.h = (bottom - top).max(0.0);
        }
        if let Some(c) = c {
            c.pop_clip();
            if list_h > h {
                let thumb = (h * h / list_h).max(24.0);
                let top = list_top + (h - thumb) * (scroll / (list_h - h)).clamp(0.0, 1.0);
                c.fill_round_rect(WIDTH - 6.0, top, 3.0, thumb, 1.5, pal.text_dim.with_alpha(0.45));
            }
        }
    }
    let not_shown = m.sessions.len() - active_shown - idle_shown;
    if not_shown > 0 {
        y += 2.0 + link(&g, &mut hits, pal, &f("+{0} more", &[&not_shown]), y + 2.0, Action::MoreSessions)?;
    } else if v.expanded && cut > 0 {
        y += 2.0 + link(&g, &mut hits, pal, t("Show fewer"), y + 2.0, Action::FewerSessions)?;
    }

    // The user's own merge and pull requests.
    if !m.mine.is_empty() {
        y = mine(&g, &mut hits, pal, &m.mine, y)?;
    }

    // The session mailbox, once its folder is known (the settings say where it's looked for).
    if let Some(mail) = m.mailbox.as_ref().filter(|m| m.root.is_some()) {
        y = mailbox(&g, &mut hits, pal, mail, m.now, y)?;
    }

    // The user's own extensions.
    for e in &m.extensions {
        y = extension(&g, &hits, pal, e, m.now, y)?;
    }

    // GPG signing key.
    if let Some(k) = m.gpg {
        y += 6.0;
        if let Some(c) = c {
            c.fill_rect(PAD, y, inner, 1.0, pal.divider);
        }
        y += 1.0 + 12.0;
        let icon = g.glyph(if k.unlocked { '\u{E785}' } else { '\u{E72E}' }, 14.0)?;
        let label = g.text(if k.unlocked { t("GPG key unlocked") } else { t("GPG key locked") }, 13.0, NORMAL, None)?;
        let right_text = match (k.unlocked, k.left) {
            (true, Some(left)) if k.at_least => f("stays so for at least {0}", &[&timefmt::countdown(left)]),
            (true, Some(left)) => f("locks in about {0}", &[&timefmt::countdown(left)]),
            (true, None) => t("for now").to_string(),
            (false, _) => t("asks for the passphrase on the next commit").to_string(),
        };
        let note = g.text(&right_text, 12.0, NORMAL, None)?;
        let color = if k.unlocked { pal.text } else { pal.text_dim };
        if let Some(c) = c {
            c.text(&icon, PAD, y + (label.height - icon.height) / 2.0, color);
            c.text(&label, PAD + icon.width + 8.0, y, color);
            c.text(&note, WIDTH - PAD - note.width, y + (label.height - note.height) / 2.0, pal.text_dim);
        }
        y += label.height + 6.0;
        if let (true, Some(left)) = (k.unlocked, k.left) {
            let fraction = (left as f32 / k.total.max(1) as f32).clamp(0.0, 1.0);
            if let Some(c) = c {
                c.fill_round_rect(PAD, y, inner, 4.0, 2.0, pal.track);
                let bar = if left <= GPG_ENDING_SECS { pal.warn } else { pal.accent };
                c.fill_round_rect(PAD, y, (inner * fraction).max(4.0), 4.0, 2.0, bar);
            }
            y += 4.0 + 6.0;
        }
    }

    // Footer links.
    if m.hidden > 0 || m.tracking == tracking::State::Off {
        y += 6.0;
    }
    if m.hidden > 0 {
        let label = if m.hidden == 1 {
            f("Show {0} hidden session", &[&m.hidden])
        } else {
            f("Show {0} hidden sessions", &[&m.hidden])
        };
        y += link(&g, &mut hits, pal, &label, y, Action::UnhideAll)?;
    }
    if m.tracking == tracking::State::Off {
        y += link(&g, &mut hits, pal, t("Turn on instant updates (hooks)"), y, Action::EnableTracking)?;
    }
    let height = y + PAD - 4.0;
    if let (Some(c), Some(tip)) = (c, hits.tip.take()) {
        draw_tip(&g, c, pal, &tip, height)?;
    }
    Ok(Layout { height, hits: hits.list, list_h })
}

/// One session row. Full rows: title and status, then the VS Code window and
/// details. Compact rows (idle sessions): one line. Returns the row's height.
#[allow(clippy::too_many_arguments, reason = "a row's session, icon, animation, position and group")]
fn session_row(
    g: &gfx::Gfx,
    hits: &mut Hits,
    pal: &Palette,
    row: &SessionRow,
    icon: Option<&gfx::Icon>,
    pulse: f32,
    y: f32,
    group: usize,
    compact: bool,
) -> Result<f32> {
    const ICON: f32 = 14.0;
    // A third line for the branch's merge request, a context getting full and
    // background work still running.
    let watching = row.watching.as_deref().filter(|_| !compact);
    let change = row.change.as_ref().filter(|_| !compact);
    let full = row.context.filter(|_| !compact);
    let h = if compact {
        RECENT_ROW_H
    } else if watching.is_some() || change.is_some() || full.is_some() {
        ROW_H + WATCH_H
    } else {
        ROW_H
    };
    let (x0, w) = (PAD / 2.0, WIDTH - PAD);
    let close = g.text("✕", 11.0, NORMAL, None)?;
    let close_w = 26.0;
    let close_x = x0 + w - close_w - 2.0;
    let row_hovered = hits.group_hovered(group);
    // Other providers' sessions can't be hidden or pinned: no buttons, but
    // Copilot's have statistics, so they get that one button.
    let buttons = row.other.is_none();
    let stats_only = row.other == Some("Copilot");
    let close_hovered = buttons && hits.next_hovered();
    // The buttons come first so they win over the row underneath.
    if buttons {
        hits.push(close_x, y + 2.0, close_w, h - 4.0, group, Action::Hide(row.id.clone()));
    }
    let stats = g.glyph('\u{E9D2}', 12.0)?;
    let stats_x = if stats_only { close_x } else { close_x - close_w };
    let stats_hovered = (buttons || stats_only) && hits.next_hovered();
    if buttons || stats_only {
        hits.push(stats_x, y + 2.0, close_w, h - 4.0, group, Action::Stats(Some(row.id.clone())));
    }
    // Pin (or unpin) to the taskbar.
    let pin = g.glyph(if row.pinned { '\u{E77A}' } else { '\u{E718}' }, 12.0)?;
    let pin_x = stats_x - close_w;
    let pin_hovered = buttons && hits.next_hovered();
    if buttons {
        hits.push(pin_x, y + 2.0, close_w, h - 4.0, group, Action::Pin(row.id.clone()));
    }

    // Archived sessions are faded, unless they need the user; the buttons are not.
    let fade = if row.archived && row.status != Status::NeedsInput { ARCHIVED_OPACITY } else { 1.0 };
    if let Some(c) = hits.canvas {
        c.set_opacity(fade);
    }

    let text_x = PAD + 16.0;
    // The request's pill comes before the row's own click area, so a click on it opens it.
    let line_y = y + ROW_H - 6.0;
    let mut pill_x = text_x - 6.0;
    if let Some(ch) = change {
        let t = g.text(&ch.text, 11.5, NORMAL, Some(WIDTH - PAD - text_x - 12.0))?;
        let (w, h) = (t.width + 12.0, t.height + 4.0);
        let hovered = ch.url.is_some() && hits.next_hovered();
        if let Some(url) = &ch.url {
            hits.push(pill_x, line_y, w, h, group, Action::Open(url.clone()));
        }
        if let Some(c) = hits.canvas {
            let color = checks_color(pal, ch.checks, ch.merged);
            c.fill_round_rect(pill_x, line_y, w, h, h / 2.0, color.with_alpha(if hovered { 0.26 } else { 0.14 }));
            c.text(&t, pill_x + 6.0, line_y + 2.0, color);
        }
        pill_x += w + 6.0;
    }
    if let Some(fill) = full {
        let percent = format!("{:.0}", fill.share * 100.0);
        let text = if fill.soon {
            f("Context {0}% full · compacts soon", &[&percent])
        } else {
            f("Context {0}% full", &[&percent])
        };
        let t = g.text(&text, 11.5, NORMAL, None)?;
        let (w, h) = (t.width + 12.0, t.height + 4.0);
        if let Some(c) = hits.canvas {
            let color = if fill.soon { pal.crit } else { pal.warn };
            c.fill_round_rect(pill_x, line_y, w, h, h / 2.0, color.with_alpha(0.14));
            c.text(&t, pill_x + 6.0, line_y + 2.0, color);
        }
        pill_x += w + 6.0;
    }
    let shown_buttons = if buttons { 3.0 } else { f32::from(u8::from(stats_only)) };
    let right = WIDTH - PAD - if row_hovered { shown_buttons * close_w } else { 0.0 };
    let icon = icon.filter(|_| row.vscode);
    // Sessions in a terminal: a prompt symbol where VS Code's icon would be.
    let term = (!row.vscode).then(|| g.glyph('\u{E756}', 12.0)).transpose()?;
    let icon_w = if icon.is_some() || term.is_some() { ICON + 5.0 } else { 0.0 };
    let status = g.text(&row.status_text, 12.0, NORMAL, None)?;
    // Running subagents: a badge before the state, whatever the state (a done
    // turn can leave background agents working).
    let agents = (row.agents > 0).then(|| g.text(&agents_label(row.agents), 11.0, NORMAL, None)).transpose()?;
    const AGENTS_GAP: f32 = 6.0;
    let agents_w = agents.as_ref().map_or(0.0, |t| t.width + 10.0 + AGENTS_GAP);
    // Ends `AGENTS_GAP` before `x_end`, centred on `mid`.
    let draw_agents = |c: &Canvas, x_end: f32, mid: f32| {
        if let Some(t) = &agents {
            let (w, h) = (t.width + 10.0, t.height + 2.0);
            let x = x_end - AGENTS_GAP - w;
            c.fill_round_rect(x, mid - h / 2.0, w, h, h / 2.0, pal.working.with_alpha(0.16));
            c.text(t, x + 5.0, mid - t.height / 2.0, pal.working);
        }
    };
    let status_w = status.width + agents_w;
    let watch = watching
        .map(|w| g.text(&f("👀 Watching: {0}", &[&w]), 11.5, NORMAL, Some((right - pill_x - 12.0).max(40.0))))
        .transpose()?;
    if let (Some(c), Some(t)) = (hits.canvas, &watch) {
        let (w, h) = (t.width + 12.0, t.height + 4.0);
        c.fill_round_rect(pill_x, line_y, w, h, h / 2.0, pal.working.with_alpha(0.14));
        c.text(t, pill_x + 6.0, line_y + 2.0, pal.working);
    }
    if let Some(c) = hits.canvas {
        if row_hovered {
            c.set_opacity(1.0);
            c.fill_round_rect(x0, y, w, h, 6.0, pal.row_hover);
            c.set_opacity(fade);
        }
    }
    let color = pal.status(row.status);
    let dot = |c: &Canvas, cy: f32| match row.status {
        Status::Working => c.stroke_circle(PAD + 4.0, cy, 3.6, 1.5, color),
        Status::Done if row.seen => c.stroke_circle(PAD + 4.0, cy, 3.6, 1.5, color),
        Status::Idle => c.stroke_circle(PAD + 4.0, cy, 3.0, 1.2, color),
        _ => c.fill_circle(PAD + 4.0, cy, 4.0, color),
    };
    let status_color = match row.status {
        Status::Idle => pal.text_dim,
        Status::NeedsInput if row.urgent => pal.crit,
        Status::Done if row.seen => color.with_alpha(0.75),
        _ => color,
    };
    // Prompt cache timer, a clock and the time left like in the Claude Code
    // extension: dim while there's time, red and glowing near the end, then a
    // faded yellow once the cache is gone (the next prompt re-reads everything).
    const CLOCK: f32 = 12.0;
    let cache = match row.cache_left {
        Some(left) if left > 0 => {
            let warn = left <= CACHE_WARN_SECS;
            Some((cache_time(left), warn, if warn { pal.crit } else { pal.text_dim }))
        }
        Some(_) if !compact => Some((t("expired").to_string(), false, pal.warn.with_alpha(0.65))),
        _ => None,
    };
    let cache = cache.map(|(t, warn, color)| g.text(&t, 12.0, NORMAL, None).map(|t| (t, warn, color))).transpose()?;
    let cache_w = cache.as_ref().map_or(0.0, |(t, _, _)| CLOCK + 4.0 + t.width + 12.0);
    let draw_cache = |c: &Canvas, x: f32, y: f32| {
        if let Some((t, warn, color)) = &cache {
            let color = *color;
            if *warn {
                let w = CLOCK + 4.0 + t.width;
                c.fill_round_rect(
                    x - 4.0,
                    y - 1.0,
                    w + 8.0,
                    t.height + 2.0,
                    4.0,
                    pal.crit.with_alpha(0.10 + 0.25 * pulse),
                );
            }
            c.clock(x + CLOCK / 2.0, y + t.height / 2.0, CLOCK / 2.0 - 0.5, color);
            c.text(t, x + CLOCK + 4.0, y, color);
        }
    };

    // "archived" tag after the window name for sessions archived in the extension.
    let tag = row.other.or_else(|| row.archived.then_some(t("archived")));
    let tag = tag.map(|t| g.text(t, 11.0, NORMAL, None)).transpose()?;
    let tag_w = tag.as_ref().map_or(0.0, |t| t.width + 14.0);
    let draw_tag = |c: &Canvas, x: f32, mid: f32| {
        if let Some(t) = &tag {
            let (w, h) = (t.width + 10.0, t.height + 2.0);
            c.stroke_round_rect(x + 0.5, mid - h / 2.0, w - 1.0, h, h / 2.0, 1.0, pal.text_dim.with_alpha(0.6));
            c.text(t, x + 5.0, mid - t.height / 2.0, pal.text_dim);
        }
    };

    if compact {
        let name = g.text(
            &row.workspace,
            13.0,
            SEMIBOLD,
            Some((right - text_x - icon_w - tag_w - status_w - 10.0).max(40.0)),
        )?;
        let line_y = y + (h - name.height) / 2.0;
        let title_x = text_x + icon_w + name.width + 8.0 + tag_w;
        let room = right - title_x - status_w - cache_w - 10.0;
        let title = (room > 24.0).then(|| g.text(&row.title, 12.0, NORMAL, Some(room))).transpose()?;
        if let Some(c) = hits.canvas {
            dot(c, line_y + name.height / 2.0);
            if let Some(icon) = icon {
                c.icon(icon, text_x, line_y + (name.height - ICON) / 2.0, ICON);
            }
            if let Some(t) = &term {
                c.text(t, text_x + (ICON - t.width) / 2.0, line_y + (name.height - t.height) / 2.0, pal.text_dim);
            }
            c.text(&name, text_x + icon_w, line_y, pal.text);
            draw_tag(c, text_x + icon_w + name.width + 6.0, line_y + name.height / 2.0);
            if let Some(t) = &title {
                c.text(t, title_x, line_y + (name.height - t.height) / 2.0, pal.text_dim);
            }
            c.text(&status, right - status.width, line_y + (name.height - status.height) / 2.0, status_color);
            draw_agents(c, right - status.width, line_y + name.height / 2.0);
            if let Some((t, _, _)) = &cache {
                draw_cache(c, right - status_w - cache_w + 4.0, line_y + (name.height - t.height) / 2.0);
            }
        }
    } else {
        let top = y + 6.0;
        let title = g.text(&row.title, 13.0, SEMIBOLD, Some((right - text_x - status_w - 10.0).max(40.0)))?;
        let second_y = top + title.height + 2.0;
        let details = (!row.details.is_empty()).then(|| g.text(&row.details, 12.0, NORMAL, None)).transpose()?;
        let details_w = details.as_ref().map_or(0.0, |d| d.width + 10.0) + cache_w;
        let name =
            g.text(&row.workspace, 12.0, NORMAL, Some((right - text_x - icon_w - tag_w - details_w).max(40.0)))?;
        if let Some(c) = hits.canvas {
            dot(c, top + title.height / 2.0);
            c.text(&title, text_x, top, pal.text);
            c.text(&status, right - status.width, top + (title.height - status.height) / 2.0, status_color);
            draw_agents(c, right - status.width, top + title.height / 2.0);
            if let Some(icon) = icon {
                c.icon(icon, text_x, second_y + (name.height - ICON) / 2.0, ICON);
            }
            if let Some(t) = &term {
                c.text(t, text_x + (ICON - t.width) / 2.0, second_y + (name.height - t.height) / 2.0, pal.text_dim);
            }
            c.text(&name, text_x + icon_w, second_y, pal.text_dim);
            draw_tag(c, text_x + icon_w + name.width + 6.0, second_y + name.height / 2.0);
            if let Some(d) = &details {
                c.text(d, right - cache_w - d.width, second_y, pal.text_dim);
            }
            draw_cache(c, right - cache_w + 12.0, second_y);
        }
    }
    if let Some(c) = hits.canvas {
        c.set_opacity(1.0);
        if row_hovered && (buttons || stats_only) {
            // Each button shows its own colour when pointed at: hide red,
            // statistics purple, pin blue (a pinned session's pin stays blue).
            let button = |t: &gfx::Text, x: f32, hovered: bool, color: crate::theme::Color, on: bool| {
                if hovered {
                    c.fill_round_rect(x, y + 4.0, close_w, h - 8.0, 4.0, color.with_alpha(0.16));
                }
                let ink = if hovered || on { color } else { pal.text_dim };
                c.text(t, x + (close_w - t.width) / 2.0, y + (h - t.height) / 2.0, ink);
            };
            if buttons {
                button(&close, close_x, close_hovered, pal.crit, false);
                button(&pin, pin_x, pin_hovered, pal.accent, row.pinned);
            }
            button(&stats, stats_x, stats_hovered, pal.plan_to, false);
        }
    }
    hits.push(x0, y, w, h, group, Action::Session(row.id.clone()));
    Ok(h)
}

/// How the flyout's weekly bar shows the days (Settings → Limits → Weekly bar).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeekBar {
    /// Seven equal columns, one per day, each filled by what its day used of its budget.
    #[default]
    Days,
    /// The bar is the week's limit, filled to its percentage, each day as wide as its
    /// budget (a day that's over as wide as what it used).
    Budgets,
}

/// Where the weekly bar's days are: each day's place (x, width), how full it is
/// and how much of it the soft "time passed" fill covers (0 to 1), and where the
/// "now" marker goes.
type BarLayout = (Vec<(f32, f32)>, Vec<f32>, Vec<f32>, f32);

/// The weekly bar: the window's days (local midnight to midnight, see
/// `forecast::day_spans`), today outlined, weekday names below, drawn as
/// [`WeekBar`] says. Pointing at a day shows its budget and use; a day that
/// used more than its budget is yellow (today: its part of the bar and its
/// outline; days before: their name). Returns the y below it.
fn week_bar(
    g: &gfx::Gfx,
    hits: &mut Hits,
    pal: &Palette,
    plan: &forecast::Plan,
    used: f64,
    style: WeekBar,
    y: f32,
) -> Result<f32> {
    const GAP: f32 = 3.0;
    const H: f32 = 6.0;
    let c = hits.canvas;
    let inner = WIDTH - 2.0 * PAD;
    // Named by the middle of the day: a day with a few hours of the next or the
    // day before is still its own calendar day.
    let names: Vec<String> = plan.days.iter().map(|d| timefmt::weekday(d.begin + (d.end - d.begin) / 2)).collect();
    let label_h = g.text(&names[0], 10.5, SEMIBOLD, None)?.height;
    let standing = plan.standing();
    let today_day = plan.today_day();
    let passed = today_day.passed(plan.now);
    let (places, fills, ghosts, marker): BarLayout = match style {
        WeekBar::Days => {
            let w = (inner - 6.0 * GAP) / 7.0;
            let places: Vec<(f32, f32)> = (0..7).map(|i| (PAD + i as f32 * (w + GAP), w)).collect();
            let fills = plan
                .days
                .iter()
                .map(|d| match (d.used, d.budget > 0.0) {
                    (Some(u), true) => (u / d.budget) as f32,
                    (Some(u), false) if u > 0.0 => 1.0,
                    _ => 0.0,
                })
                .collect();
            // The time passed: all of the days before, today's part of today.
            let ghosts = (0..7)
                .map(|i| match i.cmp(&plan.today) {
                    std::cmp::Ordering::Less => 1.0,
                    std::cmp::Ordering::Equal => passed as f32,
                    std::cmp::Ordering::Greater => 0.0,
                })
                .collect();
            let (x, w) = places[plan.today];
            (places, fills, ghosts, x + w * passed as f32)
        }
        WeekBar::Budgets => {
            // Where each day starts and ends on the week's percentage: days before
            // today from the readings, today and the days after by their budgets.
            let mut bounds = [0.0f64; 8];
            bounds[0] = plan.days[0].before.unwrap_or(0.0);
            for i in 1..=plan.today {
                let known = if i == plan.today { plan.days[i].before.or(Some(used)) } else { plan.days[i].before };
                bounds[i] = known.unwrap_or(bounds[i - 1]).max(bounds[i - 1]);
            }
            for i in plan.today..7 {
                bounds[i + 1] = bounds[i] + plan.days[i].budget.max(0.0);
            }
            let total = bounds[7].max(used).max(1e-9);
            let shown = |i: usize| bounds[i + 1] - bounds[i] > total * 1e-3;
            let gaps = ((0..7).filter(|&i| shown(i)).count().max(1) - 1) as f32 * GAP;
            let scale = (inner - gaps) / total as f32;
            let mut x = PAD;
            let mut places = Vec::with_capacity(7);
            for i in 0..7 {
                let w = if shown(i) { (bounds[i + 1] - bounds[i]) as f32 * scale } else { 0.0 };
                places.push((x, w));
                if w > 0.0 {
                    x += w + GAP;
                }
            }
            let part = |at: f64, i: usize| ((at - bounds[i]) / (bounds[i + 1] - bounds[i]).max(1e-9)).clamp(0.0, 1.0);
            let fills = (0..7).map(|i| part(used, i) as f32).collect();
            // Where today's budget, spread over the day, would be now.
            let due = bounds[plan.today] + today_day.budget.max(0.0) * passed;
            let ghosts = (0..7).map(|i| part(due, i) as f32).collect();
            let (x, w) = places[plan.today];
            (places, fills, ghosts, x + w * part(due, plan.today) as f32)
        }
    };
    for (i, name) in names.iter().enumerate() {
        let (x, w) = places[i];
        let today = i == plan.today;
        if w <= 0.0 && !today {
            continue;
        }
        let over = plan.days[i].over();
        // Today ahead of its pace (its budget spread over its hours) warns like over it.
        let warn = over || (today && plan.today_over_pace());
        let day = g.text(name, 10.5, if today { SEMIBOLD } else { NORMAL }, None)?;
        // The day and its name, a little more than the bar, to point at.
        let hovered = hits.next_hovered();
        hits.push(x - GAP / 2.0, y - 5.0, w + GAP, H + 8.0 + label_h + 2.0, 2000 + i, Action::Day(i));
        if hovered {
            let (title, note, lines) = day_tip(plan, i, name);
            hits.tip = Some(Tip { x: x + w / 2.0, y: y + H + 3.0 + label_h + 6.0, above: y - 6.0, title, note, lines });
        }
        if let Some(c) = c {
            c.fill_round_rect(x, y, w.max(H), H, H / 2.0, pal.track);
            if ghosts[i] > 0.0 {
                c.fill_round_rect(x, y, (w * ghosts[i]).max(H), H, H / 2.0, pal.accent.with_alpha(0.22));
            }
            let fill_color = match style {
                // A day within its budget is green, over it (or today over its
                // pace) yellow; red over it on a week that runs out.
                WeekBar::Days if over && standing == forecast::Pace::Out => pal.crit,
                WeekBar::Days if warn => pal.warn,
                WeekBar::Days => pal.done,
                // The week's pace: green on it, yellow over it, red when it runs out
                // before the reset. Today over its budget is yellow on a week that's on pace.
                WeekBar::Budgets if today && over && standing == forecast::Pace::Under => pal.warn,
                WeekBar::Budgets => pace_color(pal, standing),
            };
            let fill = fills[i].clamp(0.0, 1.0);
            if fill > 0.0 {
                c.fill_round_rect(x, y, (w * fill).max(H), H, H / 2.0, fill_color);
            }
            if today {
                let ring = if warn { pal.warn } else { pal.accent };
                c.stroke_round_rect(x - 1.5, y - 1.5, w.max(H) + 3.0, H + 3.0, (H + 3.0) / 2.0, 1.0, ring);
            }
            if hovered {
                c.fill_round_rect(x - 1.5, y + H + 2.0, w.max(H) + 3.0, label_h + 2.0, 4.0, pal.row_hover);
            }
            let color = match (warn, today) {
                (true, _) => pal.warn,
                (false, true) => pal.text,
                (false, false) => pal.text_dim,
            };
            // A name wider than its day is left out, except today's.
            if today || day.width <= w + GAP {
                c.text(&day, x + (w - day.width) / 2.0, y + H + 3.0, color);
            }
        }
    }
    // "Now": a small marker where the time passed (or today's due use) ends.
    if let Some(c) = c {
        c.fill_round_rect(marker - 1.5, y - 3.0, 3.0, H + 6.0, 1.5, pal.accent);
        c.fill_circle(marker, y - 4.0, 2.5, pal.accent);
    }
    Ok(y + H + 3.0 + label_h + 4.0)
}

/// A percentage of the week with one decimal (days use a few percent).
fn pct(v: f64) -> String {
    format!("{v:.1}%")
}

/// A number with one decimal, without a trailing ".0".
fn dec(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map_or_else(|| s.clone(), str::to_string)
}

/// The colour of today's use next to the weekly limit's name: yellow from 90 %
/// of its daily budget or over its pace, red over its budget.
fn today_color(pal: &Palette, plan: forecast::Plan) -> crate::theme::Color {
    let d = plan.today_day();
    if d.over() {
        pal.crit
    } else if plan.today_over_pace() || d.share().is_some_and(|s| s >= 90.0) {
        pal.warn
    } else {
        pal.text_dim
    }
}

/// What the tooltip of day `i` of the weekly bar says: its name, and when it
/// runs if that isn't midnight to midnight; its budget; what it used, green
/// within the budget (with what it saved), yellow over today's pace, red over
/// the budget; then the week after it: what the days up to it saved or
/// overspent together, against an even spread.
fn day_tip(plan: &forecast::Plan, i: usize, name: &str) -> (String, String, Vec<(String, Ink)>) {
    let d = plan.days[i];
    let title = if i == plan.today { f("{0} · today", &[&name]) } else { name.to_string() };
    // A day with hours of the day before or after it (the reset isn't at midnight): when it runs.
    let note = if (d.end - d.begin - 86_400).abs() > 3600 {
        let begin = timefmt::local_time(d.begin, true);
        let begin = begin.strip_prefix(name).map_or(begin.as_str(), str::trim_start);
        format!("({} – {})", begin, timefmt::local_time(d.end, true))
    } else {
        String::new()
    };
    let mut lines = vec![(f("Daily budget {0} of the week", &[&pct(d.budget)]), Ink::Dim)];
    match (d.used, i.cmp(&plan.today)) {
        (_, std::cmp::Ordering::Greater) if plan.budget == forecast::Budget::Adaptive => {
            lines.push((t("Like today's until today ends, then from what's left").to_string(), Ink::Dim));
        }
        (_, std::cmp::Ordering::Greater) => {}
        (Some(used), _) => {
            let fast = i == plan.today && plan.today_over_pace();
            let ink = match (d.over(), fast) {
                (true, _) => Ink::Bad,
                (false, true) => Ink::Warn,
                (false, false) => Ink::Good,
            };
            let used_line = if d.over() {
                f("Used {0} ({1} over its budget)", &[&pct(used), &pct(used - d.budget)])
            } else if i == plan.today {
                f("Used {0} ({1} left for today)", &[&pct(used), &pct(d.budget - used)])
            } else if used < d.budget - 0.05 {
                f("Used {0} ({1} under its budget)", &[&pct(used), &pct(d.budget - used)])
            } else {
                f("Used {0}", &[&pct(used)])
            };
            lines.push((used_line, ink));
            if fast && !d.over() {
                let ahead = d.ahead(plan.now).unwrap_or(0.0);
                lines.push((f("Over pace: {0} more than even by now", &[&pct(ahead)]), Ink::Warn));
            }
            if let Some(week) = plan.week_after(i) {
                // Rounds to 0.0 %: on pace.
                let even = week.abs() < 0.05;
                let mut line = match (i == plan.today, even, week > 0.0) {
                    (false, true, _) => t("Week after this day: on pace").to_string(),
                    (false, false, false) => f("Week after this day: {0} under pace", &[&pct(-week)]),
                    (false, false, true) => f("Week after this day: {0} over pace", &[&pct(week)]),
                    (true, true, _) => t("Week so far: on pace").to_string(),
                    (true, false, false) => f("Week so far: {0} under pace", &[&pct(-week)]),
                    (true, false, true) => f("Week so far: {0} over pace", &[&pct(week)]),
                };
                // Over its budget, but the days before had saved enough.
                if d.over() && week < 0.05 {
                    line = f("{0}, evened out", &[&line]);
                }
                let ink = match (even, week > 0.0) {
                    (true, _) => Ink::Dim,
                    (false, false) => Ink::Good,
                    (false, true) => Ink::Warn,
                };
                lines.push((line, ink));
            }
        }
        (None, _) => lines.push((t("Use unknown: no reading from before the day began").to_string(), Ink::Dim)),
    }
    (title, note, lines)
}

/// A small tooltip under what it's about (above it when there's no room
/// below, `bottom` being the flyout's end), kept inside the flyout.
fn draw_tip(g: &gfx::Gfx, c: &Canvas, pal: &Palette, tip: &Tip, bottom: f32) -> Result<()> {
    let max = WIDTH - PAD - 20.0;
    let title = g.text(&tip.title, 12.0, SEMIBOLD, Some(max))?;
    let note = (!tip.note.is_empty()).then(|| g.text(&tip.note, 12.0, NORMAL, Some(max))).transpose()?;
    // The note on the title's line, if it fits there.
    let beside = title.width + note.as_ref().map_or(0.0, |n| 5.0 + n.width);
    let note_beside = beside <= max;
    let head_w = if note_beside { beside } else { title.width.max(note.as_ref().map_or(0.0, |n| n.width)) };
    let lines = &tip.lines;
    let texts = lines.iter().map(|(l, _)| g.text(l, 12.0, NORMAL, Some(max))).collect::<Result<Vec<_>>>()?;
    let w = texts.iter().map(|t| t.width).fold(head_w, f32::max) + 20.0;
    let note_h = note.as_ref().filter(|_| !note_beside).map_or(0.0, |n| n.height + 1.0);
    let h = title.height + note_h + texts.iter().map(|t| t.height + 1.0).sum::<f32>() + 14.0;
    let y = if tip.y + h > bottom - 4.0 { (tip.above - h - 2.0).max(4.0) } else { tip.y };
    let x = tip.x;
    let left = (x - w / 2.0).clamp(PAD / 2.0, (WIDTH - PAD / 2.0 - w).max(PAD / 2.0));
    c.fill_round_rect(left, y + 2.0, w, h, 6.0, crate::theme::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.25 });
    c.fill_round_rect(left, y, w, h, 6.0, pal.card_hover);
    c.stroke_round_rect(left + 0.5, y + 0.5, w - 1.0, h - 1.0, 6.0, 1.0, pal.divider.with_alpha(2.0));
    c.text(&title, left + 10.0, y + 7.0, pal.text);
    let mut ly = y + 7.0 + title.height + 1.0;
    if let Some(n) = &note {
        if note_beside {
            c.text(n, left + 10.0 + title.width + 5.0, y + 7.0 + (title.height - n.height) / 2.0, pal.text_dim);
        } else {
            c.text(n, left + 10.0, ly, pal.text_dim);
            ly += n.height + 1.0;
        }
    }
    for (t, (_, ink)) in texts.iter().zip(lines) {
        let color = match ink {
            Ink::Dim => pal.text_dim,
            Ink::Good => pal.done,
            Ink::Warn => pal.warn,
            Ink::Bad => pal.crit,
        };
        c.text(t, left + 10.0, ly, color);
        ly += t.height + 1.0;
    }
    Ok(())
}

/// The colour of how the week stands: green on an even pace, yellow over it,
/// red when it runs out before the reset.
fn pace_color(pal: &Palette, p: forecast::Pace) -> crate::theme::Color {
    match p {
        forecast::Pace::Under => pal.done,
        forecast::Pace::Over => pal.warn,
        forecast::Pace::Out => pal.crit,
    }
}

/// The forecast under the weekly bar, with the pace whose colour it takes
/// (none: plain): this week's use per day and where it ends at the reset, or,
/// early in the week, the past weeks'.
fn prediction(plan: &forecast::Plan) -> (String, Option<forecast::Pace>) {
    use forecast::Pace;
    let Some(pace) = plan.pace else {
        return (t("Prediction: after a few hours of history").to_string(), None);
    };
    let rate = dec(pace);
    let head = if plan.this_week {
        f("Prediction: {0}%/day", &[&rate])
    } else {
        f("Prediction: ~{0}%/day (past weeks)", &[&rate])
    };
    let (line, tone) = match (plan.full_at, plan.at_reset) {
        (Some(full), _) => (
            f("{0} · 100% {1}", &[&head, &timefmt::local_time(full, true)]),
            Some(if plan.this_week { plan.standing().max(Pace::Over) } else { Pace::Over }),
        ),
        (None, Some(at)) => (f("{0} · ~{1}% at the reset", &[&head, &format!("{at:.0}")]), None),
        (None, None) => (head, None),
    };
    match (plan.this_week, plan.past_pace) {
        (true, Some(past)) => (f("{0} (past weeks: ~{1}%/day)", &[&line, &dec(past)]), tone),
        _ => (line, tone),
    }
}

/// "Your MRs and PRs": reference, project, title and pipeline state per row; a
/// click opens it. Returns where it ends.
fn mine(g: &gfx::Gfx, hits: &mut Hits, pal: &Palette, list: &[forge::Change], mut y: f32) -> Result<f32> {
    let inner = WIDTH - 2.0 * PAD;
    y += 6.0;
    if let Some(c) = hits.canvas {
        c.fill_rect(PAD, y, inner, 1.0, pal.divider);
    }
    y += 1.0 + 12.0;
    let head = g.text(t("Your MRs and PRs"), 14.0, SEMIBOLD, None)?;
    if let Some(c) = hits.canvas {
        c.text(&head, PAD, y, pal.text);
    }
    y += head.height + 6.0;
    for (i, ch) in list.iter().take(6).enumerate() {
        let state = match (ch.draft, ch.checks) {
            (_, Some(k)) => checks_text(ch.forge, k),
            (true, None) => t("Draft"),
            (false, None) => t("open"),
        };
        let state = g.text(state, 12.0, NORMAL, None)?;
        let reference = g.text(&ch.reference(), 13.0, SEMIBOLD, None)?;
        let project = ch.project.rsplit('/').next().unwrap_or(&ch.project);
        let room = inner - reference.width - state.width - 24.0;
        let title = g.text(&format!("{project} · {}", ch.title), 12.0, NORMAL, Some(room.max(40.0)))?;
        let h = reference.height + 8.0;
        let hovered = hits.next_hovered();
        hits.push(PAD / 2.0, y, WIDTH - PAD, h, 3000 + i, Action::Open(ch.url.clone()));
        if let Some(c) = hits.canvas {
            if hovered {
                c.fill_round_rect(PAD / 2.0, y, WIDTH - PAD, h, 6.0, pal.row_hover);
            }
            let mid = y + h / 2.0;
            c.text(&reference, PAD, mid - reference.height / 2.0, pal.text);
            c.text(&title, PAD + reference.width + 8.0, mid - title.height / 2.0, pal.text_dim);
            let color = checks_color(pal, ch.checks, false);
            c.text(&state, WIDTH - PAD - state.width, mid - state.height / 2.0, color);
        }
        y += h;
    }
    Ok(y)
}

/// One of the user's own extensions: its label as the heading (with how long
/// its file hasn't changed, when that's longer than it said is fine), then its
/// rows: a label, and a text, a time or a bar. Only to read: nothing in it
/// can be clicked. Returns where it ends.
fn extension(
    g: &gfx::Gfx,
    hits: &Hits,
    pal: &Palette,
    e: &crate::extensions::Extension,
    now: i64,
    mut y: f32,
) -> Result<f32> {
    use crate::extensions::{State, Value};
    let inner = WIDTH - 2.0 * PAD;
    let c = hits.canvas;
    y += 6.0;
    if let Some(c) = c {
        c.fill_rect(PAD, y, inner, 1.0, pal.divider);
    }
    y += 1.0 + 12.0;
    let head = g.text(&e.label, 14.0, SEMIBOLD, Some(inner * 0.6))?;
    let stale = e.stale(now);
    if let Some(c) = c {
        c.text(&head, PAD, y, pal.text);
    }
    if let Some(age) = stale {
        let note = g.text(&f("Not updated for {0}", &[&timefmt::ago(age)]), 12.0, NORMAL, None)?;
        if let Some(c) = c {
            c.text(&note, WIDTH - PAD - note.width, y + (head.height - note.height) / 2.0, pal.warn);
        }
    }
    y += head.height + 6.0;
    for row in &e.flyout {
        let label = g.text(&row.label, 13.0, NORMAL, Some(inner * 0.45))?;
        let value = g.text(&row.text(now), 12.0, NORMAL, Some(inner - label.width - 12.0))?;
        // Out of date: all of it dim, as it may no longer be so.
        let ink = match row.state {
            _ if stale.is_some() => pal.text_dim,
            State::Plain => pal.text_dim,
            State::Ok => pal.done,
            State::Warn => pal.warn,
            State::Bad => pal.crit,
        };
        if let Some(c) = c {
            c.text(&label, PAD, y, if stale.is_some() { pal.text_dim } else { pal.text });
            c.text(&value, WIDTH - PAD - value.width, y + (label.height - value.height).max(0.0) / 2.0, ink);
        }
        y += label.height.max(value.height) + 4.0;
        if let Value::Progress { fraction, .. } = row.value {
            if let Some(c) = c {
                c.fill_round_rect(PAD, y, inner, 4.0, 2.0, pal.track);
                let bar = if row.state == State::Plain && stale.is_none() { pal.accent } else { ink };
                if fraction > 0.0 {
                    c.fill_round_rect(PAD, y, (inner * fraction as f32).max(4.0), 4.0, 2.0, bar);
                }
            }
            y += 4.0 + 4.0;
        }
        y += 2.0;
    }
    Ok(y)
}

/// Most inboxes with unread messages the mailbox section lists.
const MAIL_ROWS: usize = 6;

/// The session mailbox: a heading with what's unread, a row per inbox with
/// unread messages (how many, how long the oldest has waited, the newest's
/// sender and topic), then the inboxes where everything was handled. A click
/// opens the inbox in Explorer. Returns where it ends.
fn mailbox(
    g: &gfx::Gfx,
    hits: &mut Hits,
    pal: &Palette,
    mail: &crate::mailbox::Snapshot,
    now: i64,
    mut y: f32,
) -> Result<f32> {
    let inner = WIDTH - 2.0 * PAD;
    y += 6.0;
    if let Some(c) = hits.canvas {
        c.fill_rect(PAD, y, inner, 1.0, pal.divider);
    }
    y += 1.0 + 12.0;
    let unread = mail.unread();
    let head = g.text(t("Mailbox"), 14.0, SEMIBOLD, None)?;
    let summary = match unread {
        0 => t("All handled").to_string(),
        1 => t("1 unread").to_string(),
        n => f("{0} unread", &[&n]),
    };
    let summary = g.text(&summary, 12.0, NORMAL, None)?;
    if let Some(c) = hits.canvas {
        c.text(&head, PAD, y, pal.text);
        let color = if unread > 0 { pal.warn } else { pal.text_dim };
        c.text(&summary, WIDTH - PAD - summary.width, y + (head.height - summary.height) / 2.0, color);
    }
    y += head.height + 6.0;
    let Some(root) = &mail.root else { return Ok(y) };
    for (i, inbox) in mail.inboxes.iter().filter(|b| b.unread_count > 0).take(MAIL_ROWS).enumerate() {
        let name = g.text(&inbox.name, 13.0, SEMIBOLD, None)?;
        let waited = inbox.waiting_since().map(|at| timefmt::ago(now - at)).unwrap_or_default();
        let state = f("{0} unread · {1}", &[&inbox.unread_count, &waited]);
        let state = g.text(&state, 12.0, NORMAL, None)?;
        let newest = inbox.unread.last().map(|m| match &m.from {
            Some(from) => f("{0}: {1}", &[from, &m.topic]),
            None => m.topic.clone(),
        });
        let line = newest.map(|l| g.text(&l, 12.0, NORMAL, Some(inner - 14.0))).transpose()?;
        let h = name.height + line.as_ref().map_or(0.0, |l| l.height + 1.0) + 8.0;
        let hovered = hits.next_hovered();
        hits.push(PAD / 2.0, y, WIDTH - PAD, h, 4000 + i, Action::Folder(inbox.path.clone()));
        if hovered {
            hits.tip = Some(Tip {
                x: WIDTH / 2.0,
                y: y + h + 2.0,
                above: y,
                title: f("{0} · {1} unread", &[&inbox.name, &inbox.unread_count]),
                note: String::new(),
                lines: sender_lines(inbox, now),
            });
        }
        if let Some(c) = hits.canvas {
            if hovered {
                c.fill_round_rect(PAD / 2.0, y, WIDTH - PAD, h, 6.0, pal.row_hover);
            }
            let top = y + 4.0;
            c.fill_circle(PAD + 4.0, top + name.height / 2.0, 4.0, pal.warn);
            c.text(&name, PAD + 14.0, top, pal.text);
            c.text(&state, WIDTH - PAD - state.width, top + (name.height - state.height) / 2.0, pal.warn);
            if let Some(l) = &line {
                c.text(l, PAD + 14.0, top + name.height + 1.0, pal.text_dim);
            }
        }
        y += h;
    }
    // Everything handled: one line for all of them.
    let done: usize = mail.inboxes.iter().map(|b| b.done).sum();
    if unread == 0 && !mail.inboxes.is_empty() {
        let text = f("{0} inboxes · {1} done", &[&mail.inboxes.len(), &done]);
        let line = g.text(&text, 12.0, NORMAL, Some(inner - 14.0))?;
        let h = line.height + 8.0;
        let hovered = hits.next_hovered();
        hits.push(PAD / 2.0, y, WIDTH - PAD, h, 4999, Action::Folder(root.clone()));
        if let Some(c) = hits.canvas {
            if hovered {
                c.fill_round_rect(PAD / 2.0, y, WIDTH - PAD, h, 6.0, pal.row_hover);
            }
            c.fill_circle(PAD + 4.0, y + 4.0 + line.height / 2.0, 4.0, pal.done);
            c.text(&line, PAD + 14.0, y + 4.0, pal.text_dim);
        }
        y += h;
    }
    Ok(y)
}

/// What a mailbox row's tooltip says: its unread messages by sender, with
/// each one's topic and how long it has waited (newest first, a few per sender).
fn sender_lines(inbox: &crate::mailbox::Inbox, now: i64) -> Vec<(String, Ink)> {
    const PER_SENDER: usize = 3;
    const MOST: usize = 14;
    let mut lines = Vec::new();
    for (from, messages) in inbox.by_sender() {
        let count =
            if messages.len() == 1 { t("1 message").to_string() } else { f("{0} messages", &[&messages.len()]) };
        lines.push((f("{0} · {1}", &[&from, &count]), Ink::Warn));
        for m in messages.iter().take(PER_SENDER) {
            let re = if m.reply { t("Re: ") } else { "" };
            lines.push((f("   {0}{1} · {2}", &[&re, &m.topic, &timefmt::ago(now - m.at)]), Ink::Dim));
        }
        if messages.len() > PER_SENDER {
            lines.push((f("   and {0} more", &[&(messages.len() - PER_SENDER)]), Ink::Dim));
        }
        if lines.len() >= MOST {
            break;
        }
    }
    lines
}

/// Another provider's section: a heading with its mark, name, plan and how
/// fresh the numbers are (`at`), then a bar per limit. Returns where it ends.
#[allow(clippy::too_many_arguments, reason = "a row's position, its limits and how they look")]
fn other_limits(
    g: &gfx::Gfx,
    c: Option<&Canvas>,
    pal: &Palette,
    (provider, name): (crate::widget::Provider, &str),
    plan: Option<&str>,
    at: i64,
    rows: &[(String, Option<Limit>)],
    now: i64,
    mut y: f32,
) -> Result<f32> {
    let inner = WIDTH - 2.0 * PAD;
    let (mark, color) = crate::widget::mark(provider, pal);
    let mark = g.text(mark, 13.0, SEMIBOLD, None)?;
    let title = g.text(name, 14.0, SEMIBOLD, None)?;
    let plan = plan.map(|p| g.text(p, 13.0, SEMIBOLD, None)).transpose()?;
    let as_of = g.text(&f("As of {0}", &[&timefmt::local_time(at, now - at > 20 * 3600)]), 12.0, NORMAL, None)?;
    if let Some(c) = c {
        c.fill_rect(PAD, y - 4.0, inner, 1.0, pal.divider);
        let ty = y + 8.0;
        c.text(&mark, PAD, ty + (title.height - mark.height) / 2.0, color);
        c.text(&title, PAD + mark.width + 6.0, ty, pal.text);
        if let Some(p) = &plan {
            c.text(p, PAD + mark.width + 6.0 + title.width + 8.0, ty + (title.height - p.height) / 2.0, color);
        }
        c.text(&as_of, WIDTH - PAD - as_of.width, ty + (title.height - as_of.height) / 2.0, pal.text_dim);
    }
    y += 8.0 + title.height + 10.0;
    for (label, limit) in rows {
        let Some(limit) = limit else { continue };
        let used = limit.used_at(now);
        let label = g.text(label, 13.0, NORMAL, None)?;
        let value = g.text(&format!("{}%", used.round()), 13.0, SEMIBOLD, None)?;
        let reset = match limit.resets_at {
            Some(t) if t > now => {
                f("Resets in {0} · {1}", &[&timefmt::countdown(t - now), &timefmt::local_time(t, t - now > 20 * 3600)])
            }
            Some(_) => t("Reset, nothing used since").into(),
            None => t("No reset scheduled").into(),
        };
        let reset = g.text(&reset, 12.0, NORMAL, Some(inner))?;
        let bar_y = y + label.height + 5.0;
        if let Some(c) = c {
            c.text(&label, PAD, y, pal.text);
            let value_color = if used >= 80.0 { pal.level(used) } else { pal.text };
            c.text(&value, WIDTH - PAD - value.width, y, value_color);
            c.fill_round_rect(PAD, bar_y, inner, 6.0, 3.0, pal.track);
            if used > 0.0 {
                let fill = (inner * (used as f32) / 100.0).max(6.0);
                c.fill_round_rect(PAD, bar_y, fill, 6.0, 3.0, pal.level(used));
            }
            c.text(&reset, PAD, bar_y + 10.0, pal.text_dim);
        }
        y += label.height + 5.0 + 6.0 + 4.0 + reset.height + 12.0;
    }
    Ok(y)
}

/// Time left on a cache: `52m`, then `4:09` in the last ten minutes.
pub fn cache_time(secs: i64) -> String {
    if secs > CACHE_WARN_SECS { format!("{}m", (secs + 59) / 60) } else { format!("{}:{:02}", secs / 60, secs % 60) }
}

fn summary(m: &Model) -> String {
    let count = |s: Status| m.sessions.iter().filter(|r| r.status == s).count();
    // One and several: the verb changes in German ("braucht" / "brauchen").
    let parts: Vec<String> = [
        (Status::NeedsInput, "{0} needs you", "{0} need you"),
        (Status::Working, "{0} working", "{0} working "),
        (Status::Done, "{0} done", "{0} done "),
        (Status::Idle, "{0} idle", "{0} idle "),
    ]
    .iter()
    .filter_map(|&(s, one, many)| {
        let n = count(s);
        (n > 0).then(|| f(if n == 1 { one } else { many }, &[&n]).trim_end().to_string())
    })
    .collect();
    parts.join(" · ")
}

/// The flyout window. It is created once and shown/hidden.
pub struct Flyout {
    pub hwnd: Option<HWND>,
    pub model: Option<Model>,
    hits: Vec<Hit>,
    hover: Option<usize>,
    /// The mouse in client pixels, to find the hovered hit again after a new layout.
    mouse: Option<(i32, i32)>,
    view: View,
    scale: f32,
    /// Screen position and size in pixels.
    rect: (i32, i32, i32, i32),
    /// When it was last hidden, so the click that hid it doesn't reopen it.
    hidden_at: Option<std::time::Instant>,
}

impl Flyout {
    pub const fn new() -> Flyout {
        Flyout {
            hwnd: None,
            model: None,
            hits: Vec::new(),
            hover: None,
            mouse: None,
            view: View { expanded: false, scroll: 0.0, list_max: None },
            scale: 1.0,
            rect: (0, 0, 0, 0),
            hidden_at: None,
        }
    }

    pub fn create(&mut self) {
        // SAFETY: plain window creation with a registered class.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                CLASS,
                w!("Trayward"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
        };
        if let Ok(hwnd) = hwnd {
            let round = DWMWCP_ROUND;
            // SAFETY: attribute value of the documented size.
            let _ = unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    &round as *const _ as *const _,
                    std::mem::size_of_val(&round) as u32,
                )
            };
            self.hwnd = Some(hwnd);
        }
    }

    pub fn visible(&self) -> bool {
        self.model.is_some()
    }

    /// Whether it was hidden in the last moment (by the same click).
    pub fn just_hidden(&self) -> bool {
        self.hidden_at.is_some_and(|t| t.elapsed().as_millis() < 300)
    }

    /// Shows the flyout next to `anchor` (the widget, at `dpi`), or updates it in place.
    pub fn show(&mut self, model: Model, anchor: RECT, dpi: u32, activate: bool) {
        let Some(hwnd) = self.hwnd else { return };
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        // SAFETY: Win32 calls with valid out-pointers.
        unsafe {
            let monitor = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
            let _ = GetMonitorInfoW(monitor, &mut mi);
        }
        let work = mi.rcWork;
        self.scale = dpi.max(96) as f32 / 96.0;
        let room = (work.bottom - work.top) as f32 / self.scale - 2.0 * MARGIN;
        let Some((h_dip, hits, view)) = self.layout(&model, room) else { return };
        // SAFETY: Win32 calls on our own window with valid pointers.
        unsafe {
            let dark = BOOL::from(model.dark);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const _,
                std::mem::size_of::<BOOL>() as u32,
            );
            let (w, h) = ((WIDTH * self.scale).ceil() as i32, (h_dip * self.scale).ceil() as i32);
            let margin = (MARGIN * self.scale) as i32;
            // Out of the taskbar's edge, whichever it is.
            let (x, y) = crate::taskbars::popup(&anchor, w, h, margin, false);
            self.hits = hits;
            if self.model.as_ref() != Some(&model) || activate || self.view != view {
                self.model = Some(model);
                self.view = view;
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            if activate || self.rect != (x, y, w, h) {
                self.rect = (x, y, w, h);
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_SHOWWINDOW | SWP_NOACTIVATE);
            }
            if activate {
                self.hover = None;
                self.mouse = None;
                let _ = SetForegroundWindow(hwnd);
            }
        }
        // The rows may have moved under the mouse.
        self.set_mouse(self.mouse);
    }

    /// Lays out `model` in at most `room` DIPs of height: the session list
    /// scrolls when everything doesn't fit. Returns the height, the hits and the view.
    fn layout(&self, model: &Model, room: f32) -> Option<(f32, Vec<Hit>, View)> {
        let mut view = View { list_max: None, ..self.view };
        let full = render(None, model, None, &view).ok()?;
        if full.height <= room {
            view.scroll = 0.0;
            return Some((full.height, full.hits, view));
        }
        let visible = (full.list_h - (full.height - room)).max(MIN_LIST_H.min(full.list_h));
        view.list_max = Some(visible);
        view.scroll = view.scroll.clamp(0.0, (full.list_h - visible).max(0.0));
        let l = render(None, model, None, &view).ok()?;
        Some((l.height, l.hits, view))
    }

    /// Shows all sessions, or the first few again. The caller lays it out again.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.view.expanded = expanded;
        self.view.scroll = 0.0;
    }

    /// Scrolls the session list by a mouse-wheel `delta` (120 per notch, positive:
    /// up). Returns whether it scrolls at all; then the caller lays it out again.
    pub fn wheel(&mut self, delta: i16) -> bool {
        if self.view.list_max.is_none() {
            return false;
        }
        // Clamped at the bottom by the next layout.
        self.view.scroll = (self.view.scroll - f32::from(delta) / 120.0 * WHEEL_STEP).max(0.0);
        // Drawn again in any case: the layout that follows compares with this
        // view, already scrolled, and would see no change.
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window.
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        }
        true
    }

    pub fn hide(&mut self) {
        if let (Some(hwnd), true) = (self.hwnd, self.visible()) {
            self.model = None;
            self.hover = None;
            self.mouse = None;
            // Opens collapsed and scrolled to the top next time.
            self.view = View::default();
            self.hidden_at = Some(std::time::Instant::now());
            // SAFETY: our own window.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
    }

    /// Updates the hovered row from a mouse position in client pixels.
    pub fn set_mouse(&mut self, pos: Option<(i32, i32)>) {
        self.mouse = pos;
        let hover = pos.and_then(|(x, y)| {
            let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
            crate::hits::find(&self.hits, x, y)
        });
        if hover != self.hover {
            self.hover = hover;
            if let Some(hwnd) = self.hwnd {
                // SAFETY: our own window.
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
        }
    }

    /// Pointing at something that can be clicked (not a day of the weekly bar).
    pub fn hovering(&self) -> bool {
        self.hover.and_then(|i| self.hits.get(i)).is_some_and(|h| !matches!(h.action, Action::Day(_)))
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        crate::hits::find(&self.hits, x, y).map(|i| self.hits[i].action.clone())
    }

    /// Handles WM_PAINT.
    pub fn paint(&self, hwnd: HWND) {
        // SAFETY: BeginPaint/EndPaint pair on our own window.
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            if let Some(m) = &self.model {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let pal = theme::palette(m.dark);
                let hover = self.hover.and_then(|i| self.hits.get(i).map(|h| (i, h.tag)));
                let _ = gfx::paint_window(hwnd, rc.right, rc.bottom, 96.0 * self.scale, pal.surface, |c| {
                    let _ = render(Some(c), m, hover, &self.view);
                });
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, Choice};

    const NOW: i64 = 1_794_490_000;

    fn row(i: usize, status: Status, codex: bool) -> SessionRow {
        SessionRow {
            id: format!("s{i}"),
            workspace: format!("project-{i}"),
            title: "A rather long conversation title that will not fit into one row of the flyout".into(),
            status,
            seen: false,
            status_text: "Working · 3m".into(),
            details: "Opus 5.5 · 343k ctx · 97k out".into(),
            vscode: !codex,
            archived: i == 4,
            other: codex.then_some("Codex"),
            cache_left: Some(if i == 1 { 300 } else { -60 }),
            urgent: i == 0,
            watching: (i == 2).then(|| "Wait for the CI pipeline".into()),
            agents: if codex { 0 } else { [0, 1, 3, 2, 0][i % 5] },
            context: (i == 3).then_some(crate::transcripts::Fill { share: 0.91, soon: true }),
            pinned: i == 1,
            change: (i < 3).then(|| ChangeLine {
                text: "!37 · Pipeline running".into(),
                url: Some(format!("https://example.com/{i}")),
                checks: Some(forge::Checks::Running),
                merged: false,
            }),
        }
    }

    /// Everything the flyout can show at once.
    fn model() -> Model {
        let limit = |used| Some(Limit { used, resets_at: Some(NOW + 30 * 3600) });
        Model {
            dark: true,
            now: NOW,
            weekly: limit(46.0),
            five_hour: limit(31.0),
            plan: Some("Ultra".into()),
            weekly_plan: Some(forecast::plan(&[], 46.0, NOW + 30 * 3600, NOW)),
            claude: true,
            fable: Some(crate::usage::ModelLimit {
                limit: Limit { used: 4.0, resets_at: Some(NOW + 30 * 3600) },
                share: Some(50.0),
            }),
            week_bar: WeekBar::Days,
            five_hour_simulated: false,
            updated_at: Some(NOW - 60),
            error: Some("Rate limited, retrying later.".into()),
            tracking: tracking::State::Outdated,
            sessions: [Status::NeedsInput, Status::Working, Status::Done, Status::Done, Status::Idle, Status::Working]
                .iter()
                .enumerate()
                .map(|(i, s)| row(i, *s, i == 5))
                .collect(),
            hidden: 2,
            refresh: Refresh::Ready,
            vscode_exe: None,
            pulse: 0.5,
            shine: Some(0.4),
            codex: Some(Codex {
                plan: Some("Plus".into()),
                weekly: limit(41.0),
                five_hour: limit(18.0),
                at: NOW - 600,
            }),
            copilot: Some(Copilot {
                plan: Some("Pro".into()),
                label: "Premium requests · 225 of 300 left".into(),
                limit: limit(25.0).unwrap(),
                at: NOW - 120,
            }),
            gpg: Some(Gpg { unlocked: true, left: Some(1200), total: 3600, at_least: false }),
            mailbox: None,
            extensions: Vec::new(),
            mine: vec![forge::Change {
                forge: forge::Forge::GitLab,
                number: 42,
                title: "A long merge request title that does not fit into the row".into(),
                url: "https://example.com/42".into(),
                project: "acme/shop".into(),
                state: forge::State::Open,
                draft: false,
                checks: Some(forge::Checks::Failed),
                updated: 0,
            }],
        }
    }

    #[test]
    fn lays_out_everything_in_both_languages() {
        let m = model();
        // Session titles and details may be shortened ("…"); nothing else.
        let mut may_cut: Vec<&str> =
            m.sessions.iter().flat_map(|r| [r.title.as_str(), r.details.as_str(), r.workspace.as_str()]).collect();
        let titles: Vec<String> = m.mine.iter().map(|c| format!("shop · {}", c.title)).collect();
        may_cut.extend(titles.iter().map(String::as_str));
        for lang in [Choice::English, Choice::German] {
            i18n::set(lang);
            let l = render(None, &m, None, &View::default()).unwrap();
            assert!(l.height > 400.0 && !l.hits.is_empty(), "{lang:?}: {}", l.height);
            let cut: Vec<String> = gfx::take_cut().into_iter().filter(|c| !may_cut.contains(&c.as_str())).collect();
            assert!(cut.is_empty(), "{lang:?}: cut off: {cut:?}");
        }
    }

    #[test]
    fn extensions_show_their_rows_with_nothing_to_click() {
        let mut m = model();
        let without = render(None, &m, None, &View::default()).unwrap();
        let e = crate::extensions::parse(
            r#"{"version":1,"id":"a","label":"Backup","stale_after":60,"flyout":[
                {"label":"Photos","type":"progress","value":40},
                {"label":"Next run","type":"time","value":"2026-11-18T23:00:00Z"},
                {"label":"Documents","type":"text","value":"Done","state":"ok"}]}"#,
            NOW - 10,
        )
        .unwrap();
        // The second one is out of date.
        m.extensions = vec![e.clone(), crate::extensions::Extension { changed: NOW - 3600, ..e }];
        for lang in [Choice::English, Choice::German] {
            i18n::set(lang);
            let with = render(None, &m, None, &View::default()).unwrap();
            assert!(with.height > without.height + 150.0, "{}", with.height - without.height);
            assert_eq!(with.hits.len(), without.hits.len());
        }
    }

    #[test]
    fn writes_numbers_with_one_decimal() {
        assert_eq!(dec(14.46), "14.5");
        assert_eq!(dec(15.0), "15");
        assert_eq!(dec(0.04), "0");
        assert_eq!(pct(14.0), "14.0%");
    }

    #[test]
    fn says_the_prediction_in_one_line() {
        i18n::set(Choice::English);
        let mut plan = forecast::plan(&[], 46.0, NOW + 30 * 3600, NOW);
        plan.pace = Some(7.26);
        plan.this_week = true;
        plan.past_pace = Some(6.0);
        plan.at_reset = Some(55.4);
        plan.full_at = None;
        assert_eq!(prediction(&plan), ("Prediction: 7.3%/day · ~55% at the reset (past weeks: ~6%/day)".into(), None));
        // Early in the week, by the past weeks' pace: nothing to compare with.
        plan.this_week = false;
        assert_eq!(prediction(&plan).0, "Prediction: ~7.3%/day (past weeks) · ~55% at the reset");
        plan.pace = None;
        assert_eq!(prediction(&plan).0, "Prediction: after a few hours of history");
    }

    #[test]
    fn day_tips_say_the_use_in_green_yellow_or_red() {
        i18n::set(Choice::English);
        let mut plan = forecast::plan(&[], 46.0, NOW + 30 * 3600, NOW);
        let today = plan.today;
        // Today half over, with a budget of 15 %: 7.5 % would be even by now.
        plan.days[today].begin = NOW - 12 * 3600;
        plan.days[today].end = NOW + 12 * 3600;
        plan.days[today].budget = 15.0;
        plan.days[today].used = Some(8.0);
        let (_, _, lines) = day_tip(&plan, today, "Fri");
        assert_eq!(lines[1], ("Used 8.0% (7.0% left for today)".into(), Ink::Good));
        plan.days[today].used = Some(10.0);
        let (_, _, lines) = day_tip(&plan, today, "Fri");
        assert_eq!(lines[1], ("Used 10.0% (5.0% left for today)".into(), Ink::Warn));
        assert_eq!(lines[2], ("Over pace: 2.5% more than even by now".into(), Ink::Warn));
        plan.days[today].used = Some(16.0);
        let (_, _, lines) = day_tip(&plan, today, "Fri");
        assert_eq!(lines[1], ("Used 16.0% (1.0% over its budget)".into(), Ink::Bad));
    }

    #[test]
    fn day_tips_carry_what_days_saved_over_the_week() {
        i18n::set(Choice::English);
        let mut plan = forecast::plan(&[], 46.0, NOW + 30 * 3600, NOW);
        // Each day its even share; day 0 uses 1 % less, day 1 2 % less, day 2 2 % more.
        let mut week = 0.0;
        for (i, diff) in [-1.0, -2.0, 2.0].into_iter().enumerate() {
            let d = &mut plan.days[i];
            d.budget = 100.0 * (d.end - d.begin) as f64 / (7 * 86_400) as f64;
            d.before = Some(week);
            d.used = Some(d.budget + diff);
            week += d.budget + diff;
        }
        let (_, _, lines) = day_tip(&plan, 1, "Tue");
        assert!(lines[1].0.ends_with("(2.0% under its budget)") && lines[1].1 == Ink::Good);
        assert_eq!(lines[2], ("Week after this day: 3.0% under pace".into(), Ink::Good));
        let (_, _, lines) = day_tip(&plan, 2, "Wed");
        assert!(lines[1].0.ends_with("(2.0% over its budget)") && lines[1].1 == Ink::Bad);
        assert_eq!(lines[2], ("Week after this day: 1.0% under pace, evened out".into(), Ink::Good));
        // Day 2 uses 4 % more: more than the days before saved.
        plan.days[2].used = plan.days[2].used.map(|u| u + 2.0);
        let (_, _, lines) = day_tip(&plan, 2, "Wed");
        assert_eq!(lines[2], ("Week after this day: 1.0% over pace".into(), Ink::Warn));
    }

    #[test]
    fn a_day_off_midnight_says_when_it_runs() {
        let mut plan = forecast::plan(&[], 46.0, NOW + 30 * 3600, NOW);
        plan.days[1].begin = plan.days[1].end - 86_400;
        assert_eq!(day_tip(&plan, 1, "Tue").1, "");
        plan.days[1].begin = plan.days[1].end - 26 * 3600;
        let (_, note, _) = day_tip(&plan, 1, "Tue");
        assert!(note.starts_with('(') && note.contains(" – ") && note.ends_with(')'), "{note}");
    }

    #[test]
    fn the_title_opens_the_usage_page() {
        let hits = render(None, &model(), None, &View::default()).unwrap().hits;
        let usage = hits.iter().position(|h| matches!(&h.action, Action::Open(u) if u == USAGE_PAGE));
        // In the header, after its buttons (hover goes by the order of the hits).
        assert_eq!(usage, Some(3));
        let title = &hits[3];
        assert!(title.x < PAD && title.y < PAD + 10.0);
    }

    #[test]
    fn codex_rows_have_no_buttons() {
        let m = model();
        let hits = render(None, &m, None, &View::default()).unwrap().hits;
        let hide_or_stats =
            |id: &str| hits.iter().any(|h| matches!(&h.action, Action::Hide(x) | Action::Stats(Some(x)) if x == id));
        assert!(hide_or_stats("s1"));
        assert!(!hide_or_stats("s5"));
        // Copilot's rows have statistics: that button, and no other.
        let mut m = model();
        m.sessions[5].other = Some("Copilot");
        let hover_row = render(None, &m, None, &View::default()).unwrap().hits;
        let of = |a: fn(&Action) -> bool| hover_row.iter().filter(|h| a(&h.action)).count();
        assert_eq!(of(|a| matches!(a, Action::Stats(Some(x)) if x == "s5")), 1);
        assert_eq!(of(|a| matches!(a, Action::Hide(x) | Action::Pin(x) if x == "s5")), 0);
    }

    #[test]
    fn long_lists_expand_and_scroll() {
        let mut m = model();
        m.sessions = (0..20).map(|i| row(i % 4, if i < 10 { Status::Working } else { Status::Idle }, false)).collect();
        for (i, r) in m.sessions.iter_mut().enumerate() {
            r.id = format!("s{i}");
        }
        let count = |hits: &[Hit], a: &Action| hits.iter().filter(|h| &h.action == a).count();
        let rows = |hits: &[Hit]| hits.iter().filter(|h| matches!(h.action, Action::Session(_)) && h.h > 0.0).count();
        // Collapsed: the first few rows and "+N more".
        let collapsed = render(None, &m, None, &View::default()).unwrap();
        assert_eq!(rows(&collapsed.hits), MAX_ROWS);
        assert_eq!(count(&collapsed.hits, &Action::MoreSessions), 1);
        // Expanded: all rows and "Show fewer".
        let open = View { expanded: true, ..View::default() };
        let expanded = render(None, &m, None, &open).unwrap();
        assert_eq!(rows(&expanded.hits), 20);
        assert_eq!(count(&expanded.hits, &Action::FewerSessions), 1);
        assert!(expanded.height > collapsed.height);
        // Limited to 200 DIPs: shorter, only the visible rows can be clicked.
        let limited = View { list_max: Some(200.0), ..open };
        let top = render(None, &m, None, &limited).unwrap();
        assert!((expanded.height - top.height - (expanded.list_h - 200.0)).abs() < 0.5);
        assert!(rows(&top.hits) < 6);
        let first = |l: &Layout| {
            l.hits.iter().find(|h| h.h > 0.0 && matches!(h.action, Action::Session(_))).map(|h| h.action.clone())
        };
        let scrolled = render(None, &m, None, &View { scroll: 400.0, ..limited }).unwrap();
        assert_ne!(first(&top), first(&scrolled));
        // Hidden rows keep their place in the list (hover indices stay the same).
        assert_eq!(top.hits.len(), scrolled.hits.len());
    }

    /// Lays `m` out, then draws it into memory: the drawing has the layout's
    /// height and hits, and shows something.
    fn drawn(m: &Model, hover: Option<(usize, usize)>, v: &View) -> Layout {
        let laid = render(None, m, hover, v).unwrap();
        let mut out = None;
        let px = gfx::testing::offscreen(WIDTH as i32, laid.height.ceil() as i32, 96.0, |c| {
            out = Some(render(Some(c), m, hover, v).unwrap());
        })
        .unwrap();
        assert!(gfx::testing::drew(&px));
        let out = out.unwrap();
        assert_eq!((out.height, out.hits.len()), (laid.height, laid.hits.len()));
        out
    }

    /// Draws `m` with nothing hovered, then with each of its hits hovered in turn.
    fn drawn_hovering_each(m: &Model, v: &View) -> Layout {
        let l = drawn(m, None, v);
        for (i, h) in l.hits.iter().enumerate() {
            drawn(m, Some((i, h.tag)), v);
        }
        l
    }

    /// A week half used: days before today under, over and unknown, today
    /// over its pace, a day without a budget, with a prediction.
    fn week(used: f64) -> forecast::Plan {
        let mut plan = forecast::plan(&[], used, NOW + 30 * 3600, NOW);
        let today = plan.today;
        let mut before = 0.0;
        for (i, d) in plan.days.iter_mut().enumerate().take(today) {
            d.budget = 12.0;
            d.before = Some(before);
            d.used = match i {
                0 => Some(8.0),
                1 => Some(20.0),
                2 => None,
                3 => Some(12.0),
                _ => Some(0.5),
            };
            before += d.used.unwrap_or(0.0);
        }
        plan.days[today].budget = 10.0;
        plan.days[today].used = Some(9.0);
        plan.days[today].before = Some(before);
        plan.today_used = Some(9.0);
        plan.days[6].budget = 0.0;
        // A day without a budget that used some, with no reading from before it.
        if today > 4 {
            (plan.days[4].budget, plan.days[4].before) = (0.0, None);
        }
        plan.pace = Some(9.5);
        plan.this_week = true;
        plan.past_pace = Some(7.0);
        plan.at_reset = Some(120.0);
        plan.full_at = Some(NOW + 20 * 3600);
        plan
    }

    /// The session mailbox: one inbox with many senders, one with a message
    /// without a sender, one with everything handled.
    fn mail() -> crate::mailbox::Snapshot {
        use crate::mailbox::{Inbox, Message, Snapshot};
        let msg = |from: Option<&str>, topic: &str, ago: i64, reply: bool| Message {
            path: PathBuf::from("message.md"),
            from: from.map(str::to_string),
            topic: topic.into(),
            at: NOW - ago,
            reply,
        };
        let mut busy = Vec::new();
        for (i, from) in ["alpha", "beta", "gamma", "delta", "epsilon"].iter().enumerate() {
            for j in 0..=(4 - i) {
                busy.push(msg(Some(from), "deploy image access", 600 * (i + j) as i64, j % 2 == 1));
            }
        }
        let inbox = |name: &str, unread: Vec<Message>, done| Inbox {
            name: name.into(),
            path: PathBuf::from(name),
            unread_count: unread.len(),
            unread,
            done,
            last_done: None,
        };
        Snapshot {
            root: Some(PathBuf::from("mailbox")),
            inboxes: vec![
                inbox("infra", busy, 3),
                inbox("shop", vec![msg(None, "a question", 60, false)], 0),
                // More unread than were read.
                Inbox { unread_count: 2, ..inbox("blog", Vec::new(), 0) },
                inbox("docs", Vec::new(), 5),
            ],
        }
    }

    /// An extension with a row of each kind and state.
    fn extension(changed: i64) -> crate::extensions::Extension {
        use crate::extensions::{Extension, Row, State, Value};
        let row = |label: &str, value, state| Row { label: label.into(), value, state };
        Extension {
            id: "backup".into(),
            label: "Backup".into(),
            description: String::new(),
            tooltip: Vec::new(),
            flyout: vec![
                row("Photos", Value::Progress { fraction: 0.4, text: "40%".into() }, State::Plain),
                row("Music", Value::Progress { fraction: 0.0, text: "0%".into() }, State::Warn),
                row("Next run", Value::Time(NOW + 3600), State::Ok),
                row("Last run", Value::Time(NOW - 3600), State::Bad),
                row("Documents", Value::Text("Done".into()), State::Ok),
            ],
            stale_after: Some(600),
            changed,
        }
    }

    /// [`model`] with everything else the flyout can show: the mailbox, extensions
    /// (one out of date), more merge requests, a week with a forecast.
    fn full() -> Model {
        let mut m = model();
        m.weekly_plan = Some(week(46.0));
        m.mailbox = Some(mail());
        m.extensions = vec![extension(NOW - 10), extension(NOW - 7200)];
        let change = |forge, number, draft, checks| forge::Change {
            forge,
            number,
            title: "Make it faster".into(),
            url: format!("https://example.com/{number}"),
            project: "acme/shop".into(),
            state: forge::State::Open,
            draft,
            checks,
            updated: 0,
        };
        m.mine.extend([
            change(forge::Forge::GitHub, 7, true, None),
            change(forge::Forge::GitHub, 8, false, None),
            change(forge::Forge::GitHub, 9, false, Some(forge::Checks::Passed)),
            change(forge::Forge::GitLab, 10, false, Some(forge::Checks::Canceled)),
            change(forge::Forge::GitLab, 11, false, Some(forge::Checks::Running)),
            change(forge::Forge::GitLab, 12, false, None),
        ]);
        m
    }

    #[test]
    fn draws_every_part_with_each_one_hovered_in_both_themes() {
        for dark in [true, false] {
            let m = Model { dark, ..full() };
            let l = drawn_hovering_each(&m, &View::default());
            // At most six of the user's requests; the mailbox's rows and the
            // handled inboxes open their folders; extensions add nothing to click.
            assert_eq!(l.hits.iter().filter(|h| (3000..4000).contains(&h.tag)).count(), 6);
            let folders: Vec<&Action> =
                l.hits.iter().map(|h| &h.action).filter(|a| matches!(a, Action::Folder(_))).collect();
            assert_eq!(folders.len(), 3);
            let days = l.hits.iter().filter(|h| matches!(h.action, Action::Day(_))).count();
            assert!(days >= 6, "{days}");
        }
    }

    #[test]
    fn the_header_says_how_fresh_the_numbers_are() {
        let base = Model { sessions: Vec::new(), ..model() };
        for refresh in [Refresh::Ready, Refresh::Busy(90), Refresh::Resting] {
            for updated_at in [Some(NOW - 60), None] {
                let m = Model { refresh, updated_at, ..base.clone() };
                let l = drawn(&m, None, &View::default());
                // The refresh button comes first; hovered, it says what a click does.
                assert_eq!(l.hits[0].action, Action::Refresh);
                drawn(&m, Some((0, 0)), &View::default());
            }
        }
        assert!(Refresh::Busy(0).is_busy() && !Refresh::Ready.is_busy() && !Refresh::Resting.is_busy());
        // Each plan's look, and none.
        for plan in [Some("Max 20x"), Some("Pro"), Some("Ultra"), None] {
            for shine in [None, Some(0.5)] {
                let m = Model { plan: plan.map(str::to_string), shine, ..base.clone() };
                drawn(&m, None, &View::default());
            }
        }
    }

    #[test]
    fn limits_show_in_every_state() {
        let limit = |used, resets_at| Some(Limit { used, resets_at });
        let base = Model { sessions: Vec::new(), gpg: None, codex: None, copilot: None, error: None, ..model() };
        let height = |m: &Model| drawn(m, None, &View::default()).height;
        let usual = height(&base);
        // Nothing known yet: both limits without data.
        let empty = Model { weekly: None, five_hour: None, fable: None, updated_at: None, ..base.clone() };
        assert!(height(&empty) < usual);
        // Ultra: no 5-hour limit, said at the end of the weekly line.
        let ultra = Model { five_hour: None, ..base.clone() };
        assert!(height(&ultra) < usual);
        // A simulated 5-hour limit that hasn't started, one that reset, one without a reset.
        for (five, simulated) in [
            (limit(0.0, None), true),
            (limit(31.0, Some(NOW - 10)), false),
            (limit(31.0, None), false),
            (limit(85.0, Some(NOW + 3600)), false),
            (limit(97.0, Some(NOW + 3600)), true),
        ] {
            let m = Model { five_hour: five, five_hour_simulated: simulated, ..base.clone() };
            assert_eq!(height(&m), usual);
        }
        // The weekly limit without its week's plan: a plain bar, red near the end;
        // Fable's limit without its share.
        let plain = Model {
            weekly: limit(96.0, Some(NOW + 3600)),
            weekly_plan: None,
            fable: Some(crate::usage::ModelLimit { limit: Limit { used: 0.0, resets_at: None }, share: None }),
            ..base.clone()
        };
        assert!(height(&plain) < usual);
        // Claude Code not on this PC: none of its limits.
        let no_claude = Model { claude: false, ..base.clone() };
        assert!(height(&no_claude) < height(&empty));
    }

    #[test]
    fn the_weekly_bar_shows_each_day_and_its_tip() {
        let base = Model { sessions: Vec::new(), gpg: None, codex: None, copilot: None, error: None, ..model() };
        let mut plans = Vec::new();
        // On pace (green), over it (yellow), running out (red); the forecast in its variants.
        plans.push(week(46.0));
        let mut out = week(96.0);
        out.days[1].used = Some(30.0);
        plans.push(out);
        let mut past = week(46.0);
        past.this_week = false;
        past.at_reset = Some(80.0);
        past.full_at = None;
        plans.push(past);
        let mut even = week(46.0);
        even.pace = None;
        even.today_used = None;
        plans.push(even);
        let mut quiet = week(10.0);
        (quiet.at_reset, quiet.full_at, quiet.past_pace) = (None, None, None);
        plans.push(quiet);
        // Today over its budget at its very end, on a week that's on pace.
        let mut late = week(30.0);
        let today = late.today;
        late.elapsed = 0.3;
        late.days[today].begin = NOW - 86_400;
        late.days[today].end = NOW;
        late.days[today].budget = 9.0;
        late.days[today].used = Some(9.5);
        plans.push(late);
        // At its end, today at 95 % of its budget (yellow) and well within it (dim).
        for budget in [9.5, 20.0] {
            let mut close = week(30.0);
            let today = close.today;
            close.elapsed = 0.3;
            close.days[today].begin = NOW - 86_400;
            close.days[today].end = NOW;
            close.days[today].budget = budget;
            plans.push(close);
        }
        for plan in plans {
            for style in [WeekBar::Days, WeekBar::Budgets] {
                let m = Model {
                    weekly_plan: Some(plan),
                    week_bar: style,
                    weekly: Some(Limit { used: plan.used, resets_at: Some(NOW + 30 * 3600) }),
                    ..base.clone()
                };
                let l = drawn(&m, None, &View::default());
                // Each day's tip, above the day when there's no room below it.
                for (i, h) in l.hits.iter().enumerate().filter(|(_, h)| matches!(h.action, Action::Day(_))) {
                    drawn(&m, Some((i, h.tag)), &View::default());
                }
            }
        }
    }

    #[test]
    fn day_tips_say_what_a_day_used() {
        i18n::set(Choice::English);
        let mut plan = week(46.0);
        let today = plan.today;
        let ink = |plan: &forecast::Plan, i: usize| day_tip(plan, i, "Day").2;
        // A day still to come: what its budget will be (adaptive budgets), or nothing more.
        if today < 6 {
            assert_eq!(ink(&plan, 6).len(), 2);
            plan.budget = forecast::Budget::Even;
            assert_eq!(ink(&plan, 6).len(), 1);
        }
        // A day the readings don't tell.
        assert_eq!(ink(&plan, 2)[1].1, Ink::Dim);
        // Used just its budget: no "under" or "over".
        assert_eq!(ink(&plan, 3)[1].0, "Used 12.0%");
        // Today on an even week, and today ahead of it or behind it.
        let lines = ink(&plan, today);
        assert!(lines.iter().any(|(l, _)| l.starts_with("Week so far")), "{lines:?}");
        for (used, elapsed) in [(46.0, 0.46), (40.0, 0.46), (60.0, 0.46)] {
            plan.used = used;
            plan.elapsed = elapsed;
            assert!(ink(&plan, today).iter().any(|(l, _)| l.starts_with("Week so far")));
        }
        // A day before today that used its even share: the week on pace after it.
        let (start, end) = (plan.days[0].begin, plan.days[6].end);
        let d = &mut plan.days[0];
        d.budget = 100.0 * (d.end - start) as f64 / (end - start) as f64;
        (d.before, d.used) = (Some(0.0), Some(d.budget));
        assert_eq!(ink(&plan, 0)[2], ("Week after this day: on pace".into(), Ink::Dim));
    }

    #[test]
    fn session_rows_show_every_state() {
        let base = Model { gpg: None, codex: None, copilot: None, error: None, mine: Vec::new(), ..model() };
        let mut rows = Vec::new();
        for (i, status) in [Status::NeedsInput, Status::Working, Status::Done, Status::Idle].into_iter().enumerate() {
            for variant in 0..6 {
                let mut r = row(i, status, false);
                r.id = format!("s{i}-{variant}");
                r.seen = variant % 2 == 1;
                r.urgent = variant == 2;
                r.archived = variant >= 4;
                r.vscode = variant != 3;
                r.pinned = variant == 1;
                r.details = if variant == 5 { String::new() } else { r.details };
                r.cache_left = [Some(3000), Some(120), Some(-5), None, Some(0), Some(59 * 60)][variant];
                r.agents = variant % 3;
                r.watching = (variant == 3).then(|| "Wait for the deploy".into());
                r.context = match variant {
                    1 => Some(crate::transcripts::Fill { share: 0.72, soon: false }),
                    2 => Some(crate::transcripts::Fill { share: 0.95, soon: true }),
                    _ => None,
                };
                r.change = match variant {
                    0 => Some(ChangeLine { text: "!37 · Merged".into(), url: None, checks: None, merged: true }),
                    3 => Some(ChangeLine {
                        text: "#8 · Checks passed".into(),
                        url: Some("https://example.com/8".into()),
                        checks: Some(forge::Checks::Passed),
                        merged: false,
                    }),
                    4 => Some(ChangeLine {
                        text: "!9 · Pipeline failed".into(),
                        url: Some("https://example.com/9".into()),
                        checks: Some(forge::Checks::Failed),
                        merged: false,
                    }),
                    5 => Some(ChangeLine {
                        text: "!10 · open".into(),
                        url: Some("https://example.com/10".into()),
                        checks: Some(forge::Checks::Canceled),
                        merged: false,
                    }),
                    _ => None,
                };
                rows.push(r);
            }
        }
        // Other providers' sessions: Codex's without buttons, Copilot's with statistics only.
        for (i, other) in [(0, "Codex"), (1, "Copilot")] {
            for status in [Status::Working, Status::Idle] {
                let mut r = row(i, status, true);
                r.id = format!("{other}-{status:?}");
                r.other = Some(other);
                rows.push(r);
            }
        }
        let n = rows.len();
        let m = Model { sessions: rows, ..base };
        let open = View { expanded: true, ..View::default() };
        let l = drawn_hovering_each(&m, &open);
        let rows_shown = l.hits.iter().filter(|h| matches!(h.action, Action::Session(_))).count();
        assert_eq!(rows_shown, n);
        // Only the pills with an address open something, and compact rows have none.
        let pills =
            l.hits.iter().filter(|h| matches!(&h.action, Action::Open(u) if u.starts_with("https://example.com/")));
        let with_url = |r: &&SessionRow| r.status != Status::Idle && r.change.as_ref().is_some_and(|c| c.url.is_some());
        assert_eq!(pills.count(), m.sessions.iter().filter(with_url).count());
        // In a terminal or in VS Code, with its icon.
        windows_com();
        let with_icon = Model { vscode_exe: Some(gfx::testing::system_exe()), ..m };
        let l = drawn(&with_icon, None, &open);
        for (i, h) in l.hits.iter().enumerate().filter(|(_, h)| matches!(h.action, Action::Session(_))).step_by(5) {
            drawn(&with_icon, Some((i, h.tag)), &open);
        }
    }

    /// COM for this test's thread, which loading an icon needs (the app has it on its UI thread).
    fn windows_com() {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // SAFETY: plain initialisation of this thread; a second call just says so.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    }

    #[test]
    fn the_list_scrolls_with_a_thumb_and_links() {
        let mut m = Model { gpg: None, mine: Vec::new(), ..model() };
        m.sessions = (0..20).map(|i| row(i % 4, if i < 10 { Status::Working } else { Status::Idle }, false)).collect();
        for (i, r) in m.sessions.iter_mut().enumerate() {
            r.id = format!("s{i}");
        }
        let open = View { expanded: true, list_max: Some(200.0), scroll: 300.0 };
        let l = drawn_hovering_each(&m, &open);
        // Rows out of view can't be clicked.
        assert!(l.hits.iter().any(|h| matches!(h.action, Action::Session(_)) && h.h == 0.0));
        drawn_hovering_each(&m, &View::default());
        // Room for all of it: no thumb.
        drawn(&m, None, &View { list_max: Some(10_000.0), ..open });
        // Only idle sessions: "Recent" right at the top.
        let idle =
            Model { sessions: m.sessions.iter().filter(|r| r.status == Status::Idle).cloned().collect(), ..m.clone() };
        drawn(&idle, None, &View::default());
        // None at all.
        let none = Model { sessions: Vec::new(), hidden: 1, tracking: tracking::State::Off, ..m.clone() };
        let l = drawn_hovering_each(&none, &View::default());
        assert!(l.hits.iter().any(|h| h.action == Action::UnhideAll));
        assert!(l.hits.iter().any(|h| h.action == Action::EnableTracking));
        let on = Model { tracking: tracking::State::On, hidden: 0, ..none };
        assert!(
            !drawn(&on, None, &View::default())
                .hits
                .iter()
                .any(|h| matches!(h.action, Action::EnableTracking | Action::UnhideAll))
        );
    }

    #[test]
    fn the_gpg_key_says_how_long_it_stays_unlocked() {
        let base = Model { sessions: Vec::new(), ..model() };
        for gpg in [
            Gpg { unlocked: true, left: Some(1200), total: 3600, at_least: true },
            Gpg { unlocked: true, left: Some(60), total: 3600, at_least: false },
            Gpg { unlocked: true, left: Some(0), total: 0, at_least: false },
            Gpg { unlocked: true, left: None, total: 3600, at_least: false },
            Gpg { unlocked: false, left: None, total: 3600, at_least: false },
        ] {
            let m = Model { gpg: Some(gpg), ..base.clone() };
            let with = drawn(&m, None, &View::default()).height;
            let without = drawn(&Model { gpg: None, ..m }, None, &View::default()).height;
            assert!(with > without + 20.0);
        }
    }

    #[test]
    fn other_providers_show_their_limits() {
        let base = Model { sessions: Vec::new(), gpg: None, ..model() };
        let limit = |used, resets_at| Some(Limit { used, resets_at });
        let codex = |weekly, five_hour, at| Codex { plan: None, weekly, five_hour, at };
        let height =
            |cx| drawn(&Model { codex: Some(cx), copilot: None, ..base.clone() }, None, &View::default()).height;
        // A day old, with one limit; near the end of both; one reset, one without data.
        let one = height(codex(limit(41.0, Some(NOW + 3600)), None, NOW - 86_400));
        let two = height(codex(limit(85.0, Some(NOW + 3600)), limit(97.0, None), NOW));
        assert!(two > one + 20.0, "{two} vs {one}");
        assert_eq!(height(codex(limit(50.0, Some(NOW - 10)), None, NOW)), one);
        assert_eq!(height(codex(None, limit(0.0, None), NOW)), one);
        let copilot = Copilot {
            plan: None,
            label: "Premium requests".into(),
            limit: Limit { used: 100.0, resets_at: Some(NOW + 40 * 86_400) },
            at: NOW,
        };
        drawn(&Model { copilot: Some(copilot), codex: None, claude: false, ..base }, None, &View::default());
    }

    #[test]
    fn the_mailbox_lists_inboxes_with_unread_messages() {
        let base = Model { sessions: Vec::new(), gpg: None, codex: None, copilot: None, ..model() };
        let height = |mailbox| drawn(&Model { mailbox, ..base.clone() }, None, &View::default()).height;
        let without = height(None);
        // Not found yet: left out.
        assert_eq!(height(Some(crate::mailbox::Snapshot::default())), without);
        i18n::set(Choice::English);
        let all = mail();
        let lines = sender_lines(&all.inboxes[0], NOW);
        // Three messages per sender, the rest counted; the senders with the
        // most first, until the tip has fourteen lines.
        assert_eq!(lines.len(), 14);
        assert_eq!(lines[0], ("alpha · 5 messages".into(), Ink::Warn));
        assert_eq!(lines[4], ("   and 2 more".into(), Ink::Dim));
        assert!(lines.iter().any(|(l, _)| l.starts_with("   Re: ")));
        // One unread message; everything handled.
        let mut one = mail();
        one.inboxes.remove(0);
        let mut handled = mail();
        for inbox in &mut handled.inboxes {
            inbox.unread.clear();
            inbox.unread_count = 0;
        }
        let m = Model { mailbox: Some(handled.clone()), ..base.clone() };
        let l = drawn_hovering_each(&m, &View::default());
        assert!(l.hits.iter().any(|h| h.action == Action::Folder(PathBuf::from("mailbox"))));
        assert!(height(Some(one)) > without);
        // A mailbox without inboxes: just its heading.
        let empty = crate::mailbox::Snapshot { root: Some(PathBuf::from("mailbox")), inboxes: Vec::new() };
        assert!(height(Some(empty)) > without);
        drawn_hovering_each(&Model { mailbox: Some(all), ..base }, &View::default());
    }

    #[test]
    fn names_states_and_counts() {
        i18n::set(Choice::English);
        assert_eq!(agents_label(1), "1 agent");
        assert_eq!(agents_label(3), "3 agents");
        let all = [forge::Checks::Running, forge::Checks::Passed, forge::Checks::Failed, forge::Checks::Canceled];
        let texts: Vec<&str> = [forge::Forge::GitLab, forge::Forge::GitHub]
            .into_iter()
            .flat_map(|f| all.map(|c| checks_text(f, c)))
            .collect();
        assert_eq!(texts.len(), 8);
        assert!(texts.iter().all(|t| !t.is_empty()));
        let pal = theme::palette(true);
        assert_eq!(checks_color(pal, None, true), pal.plan_to);
        assert_eq!(checks_color(pal, Some(forge::Checks::Canceled), false), pal.text_dim);
        assert_eq!(cache_time(3000), "50m");
        assert_eq!(cache_time(249), "4:09");
        let m = model();
        assert_eq!(summary(&m), "1 needs you · 2 working · 2 done · 1 idle");
        let two = Model { sessions: vec![row(0, Status::NeedsInput, false), row(1, Status::NeedsInput, false)], ..m };
        assert_eq!(summary(&two), "2 need you");
    }

    #[test]
    fn the_window_scrolls_hovers_and_hits_without_showing() {
        let mut f = Flyout::new();
        // Without its window class (the app registers it when it starts): no window.
        f.create();
        assert!(f.hwnd.is_none());
        assert!(!f.visible() && !f.just_hidden());
        // Without a window, nothing shows.
        f.show(model(), RECT::default(), 96, true);
        assert!(!f.visible());
        f.hide();
        // Fits: no scrolling.
        let m = model();
        let (h, hits, view) = f.layout(&m, 10_000.0).unwrap();
        assert!(h > 400.0 && view.list_max.is_none() && view.scroll == 0.0);
        assert!(!f.wheel(-120));
        // Doesn't fit: the session list scrolls, down and back up, never past its ends.
        f.view.scroll = 1_000.0;
        let (short, _, view) = f.layout(&m, 300.0).unwrap();
        assert!(short < h && view.list_max.is_some());
        assert!(view.scroll < 1_000.0);
        f.view = view;
        assert!(f.wheel(120));
        assert!(f.wheel(-240));
        assert!(f.view.scroll > 0.0);
        f.set_expanded(true);
        assert!(f.view.expanded && f.view.scroll == 0.0);
        // The mouse over the refresh button: hovering something to click.
        f.hits = hits;
        f.scale = 1.5;
        let r = &f.hits[0];
        let (x, y) = (((r.x + 1.0) * 1.5) as i32, ((r.y + 1.0) * 1.5) as i32);
        f.set_mouse(Some((x, y)));
        assert!(f.hovering());
        assert_eq!(f.hit(x, y), Some(Action::Refresh));
        // Over a day of the weekly bar: hovered, but nothing to click.
        let day = f.hits.iter().find(|h| matches!(h.action, Action::Day(_))).map(|h| (h.x + 1.0, h.y + 1.0)).unwrap();
        f.set_mouse(Some(((day.0 * 1.5) as i32, (day.1 * 1.5) as i32)));
        assert!(!f.hovering());
        f.set_mouse(None);
        assert!(!f.hovering());
        assert_eq!(f.hit(-10, -10), None);
    }
}
