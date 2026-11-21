//! The widget inside the taskbar.
//!
//! Windows 11 has no supported way to add things to the taskbar, so the widget
//! is a small layered window made a child of the taskbar (`Shell_TrayWnd`) and
//! placed just left of the notification area. A one-second check keeps it in
//! place when the tray changes width, and it is recreated when Explorer restarts.
//!
//! Layout, two rows like the clock:
//!
//! ```text
//!  ● 1
//!  ● 2   W   ━━━──  42%
//!  ○ 3   ✦ Ultra            (5h  ━────  17%  on plans with a 5-hour limit)
//! ```
//! The column on the left counts sessions that need you (yellow), are done
//! (green) and are working (blue), always in that order; a state with no
//! sessions leaves its place empty.
//!
//! Slim (display mode, Settings → Taskbar or the right-click menu): the session dots in a row on top, one limit
//! below, taking turns with the others every few seconds:
//!
//! ```text
//!  ● 1  ● 2  ○ 3
//!  W   ━━━──  42%           (then 5h, then Codex's, …)
//! ```

use crate::forecast::Pace;
use crate::gfx::{self, Canvas, NORMAL, SEMIBOLD};
use crate::sessions::Status;
use crate::settings;
use crate::taskbars::{self, Display, Taskbar};
use crate::theme::{self, Color, Palette};
use crate::uia;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CLR_INVALID, GetDC, GetPixel, MONITOR_DEFAULTTONEAREST, MonitorFromWindow, ReleaseDC,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, FindWindowExW, GW_CHILD, GetForegroundWindow, GetWindow, GetWindowRect, HWND_TOP,
    IsWindow, IsWindowVisible, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

pub const CLASS: PCWSTR = w!("Trayward.Widget");

/// Everything the widget shows. A redraw happens only when this changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub dark: bool,
    pub hover: bool,
    /// Used percent, rounded.
    pub weekly: Option<u8>,
    pub five_hour: Option<u8>,
    /// Subscription plan, e.g. "Ultra". Shown in place of the 5-hour row on
    /// plans without a 5-hour limit.
    pub plan: Option<String>,
    /// The numbers are old (last refresh failed a while ago).
    pub stale: bool,
    /// Sessions that need you, are done and are working (see [`STATES`]).
    pub sessions: [usize; 3],
    /// How the needs-you dot asks for attention.
    pub attention: Attention,
    /// Show the small usage bars (set by [`Bar::update`] from the options).
    pub bars: bool,
    /// How long the GPG signing key stays unlocked, if known and shown.
    pub gpg_left: Option<GpgLeft>,
    /// Solid background that covers taskbar buttons underneath.
    pub background: Option<Color>,
    /// Where the shine on the Ultra plan's name is (see `plan::shine`).
    pub shine: Option<f32>,
    /// Time left until the weekly and the 5-hour limit reset, shown instead
    /// of the percentage once a limit is nearly used up (e.g. "2h 10m").
    pub countdown: [Option<String>; 2],
    /// Show Claude's limits (the fields above).
    pub claude: bool,
    /// Codex's limits, when shown.
    pub codex: Option<Limits>,
    /// Copilot's monthly limit, when shown.
    pub copilot: Option<Monthly>,
    /// A session the user pinned to the taskbar.
    pub pinned: Option<Pinned>,
    /// The system monitor, when any of its values is on.
    pub monitor: Option<Monitor>,
    /// Slim: sessions on top, one limit below. Which of the limits to show
    /// (counts up; wraps around the limits there are).
    pub slim: Option<usize>,
    /// The clock's turn (counts up every few seconds while limits take turns):
    /// for a widget that goes slim by itself to fit the Widgets button.
    pub turn: usize,
    /// On a taskbar at the left or right: everything stacked in a column (set
    /// by [`Bar::update`]).
    pub column: bool,
    /// On a taskbar at the left or right, in a column or not: the background and
    /// the hover highlight keep off its sides.
    pub side: bool,
    /// Today's limit: what today used, as a percentage of its share of the
    /// week (over 100: more than its share). Set while limits take turns.
    pub day: Option<u8>,
    /// The second row shows today's limit now, in its turn with the 5-hour
    /// limit (or the plan's name).
    pub day_turn: bool,
    /// How the week's pace stands, for the weekly limit's colour.
    pub weekly_pace: Option<Pace>,
    /// Today is over its pace (its budget spread over its hours), for today's
    /// limit's colour.
    pub day_over_pace: bool,
}

/// The label of today's limit.
pub const DAY_LABEL: &str = "D";

/// A limit's colour: the weekly and 5-hour limits turn yellow at 80 % and
/// red at 95 %; today's limit is a share of a day's budget, yellow once it's
/// over 100 % or over today's pace (`pace` Over). Claude's weekly limit goes by the week's pace (which includes
/// the 80 and 95 %): yellow over an even pace, red when it runs out before the reset.
fn limit_color(pal: &Palette, label: &str, level: f64, pace: Option<Pace>) -> Color {
    match (label, pace) {
        (DAY_LABEL, p) if level > 100.0 || p.is_some_and(|p| p > Pace::Under) => pal.warn,
        (DAY_LABEL, _) => pal.bar,
        (_, Some(Pace::Out)) => pal.crit,
        (_, Some(Pace::Over)) => pal.warn,
        (_, Some(Pace::Under)) => pal.bar,
        (_, None) => pal.level(level),
    }
}

/// Today's limit's pace: Over when today is over its pace.
fn day_pace(m: &Model) -> Option<Pace> {
    m.day_over_pace.then_some(Pace::Over)
}

/// Whether a limit's value stands out (in its colour rather than the text's).
fn stands_out(label: &str, level: f64, pace: Option<Pace>) -> bool {
    match (label, pace) {
        (DAY_LABEL, p) => level > 100.0 || p.is_some_and(|p| p > Pace::Under),
        (_, Some(p)) => p > Pace::Under,
        (_, None) => level >= 80.0,
    }
}

/// The system monitor as the widget shows it: columns of values, left to
/// right (network and disk, then CPU, memory and GPU); only those switched on.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub columns: Vec<Vec<Cell>>,
}

/// One value of the system monitor.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub label: &'static str,
    pub value: String,
    /// The widest the value can get, so the widget keeps its width.
    pub widest: &'static [&'static str],
    /// Percent, for values that turn red near 100.
    pub level: Option<f32>,
    /// The last values scaled to 0..1, oldest first; empty without graphs.
    pub history: Vec<f32>,
}

/// How many seconds a graph shows.
pub const HISTORY: usize = 30;
const MON_FONT: f32 = 9.0;
const MON_ROW: f32 = 12.0;
const MON_COL_GAP: f32 = 8.0;

/// A pinned session as the widget shows it: a dot and its name, and below
/// its state or its cache timer.
#[derive(Clone, Debug, PartialEq)]
pub struct Pinned {
    pub name: String,
    pub status: Status,
    /// e.g. "Working · 3m" or "🕒 52m".
    pub line: String,
    /// Has needed the user for long, or its cache is about to run out.
    pub urgent: bool,
}

/// How wide a pinned session's name may get.
const PIN_W: f32 = 96.0;

/// One provider's limits as the widget shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub weekly: Option<u8>,
    pub five_hour: Option<u8>,
    pub countdown: [Option<String>; 2],
}

/// Copilot's monthly limit as the widget shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Monthly {
    pub used: Option<u8>,
    pub countdown: Option<String>,
    /// The line below it, e.g. "199 left".
    pub note: String,
}

/// Whose limits a block of the widget shows; with several blocks, each gets a mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
    Copilot,
}

/// The small mark in front of a provider's block: a symbol and its colour.
/// (Plain symbols, not the companies' logos.)
pub fn mark(p: Provider, pal: &Palette) -> (&'static str, Color) {
    match p {
        Provider::Claude => ("✻", pal.plan_from), // ✳ would be an emoji
        Provider::Codex => ("⬡", pal.codex),
        Provider::Copilot => ("◈", pal.copilot),
    }
}

/// Animation of the needs-you dot: it glows, shakes now and then, and turns
/// red once a session has waited too long.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Attention {
    pub urgent: bool,
    /// Glow strength, 0 to 1.
    pub glow: f32,
    /// Sideways offset in DIPs.
    pub shake: f32,
    /// A finished session's prompt cache is about to expire: the green dot turns red.
    pub done_expiring: bool,
    /// Glow strength of the done dot while it warns, 0 to 1.
    pub done_glow: f32,
}

/// How the widget fits into the taskbar, and on which taskbars it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub bars: settings::Bars,
    pub solid: bool,
    pub place: settings::Place,
    pub taskbars: settings::Taskbars,
    /// Display ids for [`settings::Taskbars::Chosen`].
    pub chosen: Vec<String>,
}

const FONT: f32 = 11.0;
const ROW_H: f32 = 16.0;
const ROW_GAP: f32 = 1.0;
const PAD_X: f32 = 6.0;
const BAR_W: f32 = 24.0;
const BAR_H: f32 = 4.0;
const LABEL_GAP: f32 = 5.0;
const VALUE_GAP: f32 = 4.0;
/// The states counted in the session column, top to bottom.
pub const STATES: [Status; 3] = [Status::NeedsInput, Status::Done, Status::Working];
const STATE_ROW_H: f32 = 11.0;
const STATE_FONT: f32 = 9.0;
const DOT_R: f32 = 3.0;
const DOT_GAP: f32 = 5.0;
/// Size of a provider's mark.
const MARK_FONT: f32 = 10.0;

enum Row {
    /// Label, used percent, the countdown to show instead, and how the
    /// week's pace stands (Claude's weekly limit).
    Limit(&'static str, Option<u8>, Option<String>, Option<Pace>),
    Plan(String),
    /// A plain line.
    Note(String),
}

/// Lays out the widget for a taskbar `height` DIPs high and returns its width.
/// With a canvas it also draws.
pub fn render(c: Option<&Canvas>, m: &Model, height: f32) -> windows::core::Result<f32> {
    let g = gfx::get()?;
    let pal: &Palette = theme::palette(m.dark);
    let top = ((height - (2.0 * ROW_H + ROW_GAP)) / 2.0).round();
    let row_mid = [top + ROW_H / 2.0, top + ROW_H + ROW_GAP + ROW_H / 2.0];
    if let Some(turn) = m.slim {
        return slim(c, &g, pal, m, height, row_mid, turn);
    }
    let mut x = PAD_X;

    // The system monitor first, apart from Claude's things.
    if let Some(mon) = m.monitor.as_ref().filter(|mon| mon.columns.iter().any(|c| !c.is_empty())) {
        x = monitor_block(c, &g, pal, mon, x, height)? + DOT_GAP;
        if let Some(c) = c {
            c.fill_rect(x, top + 3.0, 1.0, 2.0 * ROW_H + ROW_GAP - 6.0, pal.divider);
        }
        x += 1.0 + DOT_GAP;
    }

    // Sessions: one row per state, with its count.
    if m.sessions.iter().any(|&n| n > 0) {
        let col_top = ((height - 3.0 * STATE_ROW_H) / 2.0).round();
        let counts = m
            .sessions
            .iter()
            .map(|&n| (n > 0).then(|| g.text(&n.to_string(), STATE_FONT, SEMIBOLD, None)).transpose())
            .collect::<windows::core::Result<Vec<_>>>()?;
        let count_w = counts.iter().flatten().map(|t| t.width).fold(0.0, f32::max);
        if let Some(c) = c {
            for (i, (status, count)) in STATES.iter().zip(&counts).enumerate() {
                let Some(count) = count else { continue };
                let cy = col_top + STATE_ROW_H * (i as f32 + 0.5);
                state_dot(c, pal, m, *status, x + DOT_R, cy);
                c.text(count, x + 2.0 * DOT_R + 3.0, cy - count.height / 2.0, pal.text_dim);
            }
        }
        x += 2.0 * DOT_R + 3.0 + count_w + DOT_GAP;
        if let Some(c) = c {
            c.fill_rect(x, top + 3.0, 1.0, 2.0 * ROW_H + ROW_GAP - 6.0, pal.divider);
        }
        x += 1.0 + DOT_GAP;
    }

    // A pinned session: dot and name, then its state.
    if let Some(p) = &m.pinned {
        let name = g.text(&p.name, FONT, SEMIBOLD, Some(PIN_W))?;
        let line = g.text(&p.line, FONT - 1.0, NORMAL, Some(PIN_W))?;
        let tx = x + 2.0 * DOT_R + 4.0;
        if let Some(c) = c {
            let color = if p.urgent { pal.crit } else { pal.status(p.status) };
            let cy = row_mid[0];
            match p.status {
                Status::Working => c.stroke_circle(x + DOT_R, cy, DOT_R - 0.6, 1.3, color),
                _ => c.fill_circle(x + DOT_R, cy, DOT_R, color),
            }
            c.text(&name, tx, cy - name.height / 2.0, pal.text);
            c.text(&line, tx, row_mid[1] - line.height / 2.0, if p.urgent { pal.crit } else { pal.text_dim });
        }
        x = tx + name.width.max(line.width) + DOT_GAP;
        if let Some(c) = c {
            c.fill_rect(x, top + 3.0, 1.0, 2.0 * ROW_H + ROW_GAP - 6.0, pal.divider);
        }
        x += 1.0 + DOT_GAP;
    }

    // Usage: a block per provider shown, with a mark in front when there are several.
    let others = m.codex.is_some() || m.copilot.is_some();
    let mut blocks = Vec::new();
    // The second row of Claude's block when it isn't today's limit's turn; the
    // block is measured with both, so the turns don't change its width.
    let mut also = None;
    if m.claude || !others {
        let usual = match (&m.plan, m.five_hour) {
            (Some(plan), None) => Row::Plan(plan.clone()),
            _ => Row::Limit("5h", m.five_hour, m.countdown[1].clone(), None),
        };
        let day = m.day.map(|d| Row::Limit(DAY_LABEL, Some(d), None, day_pace(m)));
        let second = match day {
            Some(day) if m.day_turn => {
                also = Some(usual);
                day
            }
            Some(day) => {
                also = Some(day);
                usual
            }
            None => usual,
        };
        blocks.push((Provider::Claude, [Row::Limit("W", m.weekly, m.countdown[0].clone(), m.weekly_pace), second]));
    }
    if let Some(cx) = &m.codex {
        let rows = [
            Row::Limit("W", cx.weekly, cx.countdown[0].clone(), None),
            Row::Limit("5h", cx.five_hour, cx.countdown[1].clone(), None),
        ];
        blocks.push((Provider::Codex, rows));
    }
    if let Some(cp) = &m.copilot {
        let rows = [Row::Limit("M", cp.used, cp.countdown.clone(), None), Row::Note(cp.note.clone())];
        blocks.push((Provider::Copilot, rows));
    }
    // Marks tell the blocks apart; a lone other block gets one too (Claude's is the usual).
    let marked = blocks.len() > 1 || others;
    let mut right = x;
    for (i, (provider, rows)) in blocks.iter().enumerate() {
        if i > 0 {
            x = right + DOT_GAP;
            if let Some(c) = c {
                c.fill_rect(x, top + 3.0, 1.0, 2.0 * ROW_H + ROW_GAP - 6.0, pal.divider);
            }
            x += 1.0 + DOT_GAP;
        }
        if marked {
            let (symbol, color) = mark(*provider, pal);
            let t = g.text(symbol, MARK_FONT, SEMIBOLD, None)?;
            if let Some(c) = c {
                c.text(&t, x, (height - t.height) / 2.0, color.with_alpha(if m.stale { 0.5 } else { 1.0 }));
            }
            x += t.width + 3.0;
        }
        let measure = if *provider == Provider::Claude { also.as_ref() } else { None };
        right = limits_block(c, &g, pal, m, x, row_mid, rows, measure)?;
    }
    Ok(right + PAD_X)
}

/// The column widget's inset from the taskbar's sides: a tenth of its width, like
/// the tray's icons below it, within these bounds.
const COL_PAD: (f32, f32) = (5.0, 16.0);
/// The widest the column's lines get, on a taskbar with labels: a label and its
/// value stay close enough to read as one.
const COL_MAX: f32 = 132.0;
/// A line of the column widget.
const COL_ROW: f32 = 15.0;

/// The widget on a taskbar at the left or right edge, `width` DIPs wide:
/// everything stacked, top to bottom: the system monitor's values, the
/// session dots (a line per state), then every limit on a line of its own
/// (label left, value right; no bars, there's no room), with the provider's
/// mark above its limits when there are several. Where a part fits as at the
/// bottom (on a taskbar with labels), it's drawn so: the monitor's columns side
/// by side, the session dots next to the limits. Returns its height.
fn column(c: Option<&Canvas>, m: &Model, width: f32) -> windows::core::Result<f32> {
    let g = gfx::get()?;
    let pal: &Palette = theme::palette(m.dark);
    let inner = (width - 2.0 * (width * 0.1).clamp(COL_PAD.0, COL_PAD.1)).clamp(1.0, COL_MAX);
    let left = ((width - inner) / 2.0).round();
    let right = left + inner;
    // A part's lines all side by side, or all stacked once one of them doesn't fit.
    let stacked = |lines: &[(gfx::Text, gfx::Text)]| lines.iter().any(|(l, v)| l.width + 3.0 + v.width > inner);
    let fade = if m.stale { 0.5 } else { 1.0 };
    let mut y = 6.0;
    let mut sections = 0;
    // A divider between the parts.
    let mut part = |y: &mut f32| {
        if sections > 0 {
            if let Some(c) = c {
                c.fill_rect(left + 2.0, *y + 2.0, right - left - 4.0, 1.0, pal.divider);
            }
            *y += 6.0;
        }
        sections += 1;
    };

    // The parts below the monitor as at the bottom, two lines high, if they fit.
    let lower = Model { monitor: None, column: false, ..m.clone() };
    let has_lower = m.sessions.iter().any(|&n| n > 0) || !slim_limits(m).is_empty();
    let lower_w = render(None, &lower, LINES_H)?;
    let lower_fits = has_lower && lower_w + 8.0 <= width;

    let mon = m.monitor.as_ref().filter(|mon| mon.columns.iter().any(|c| !c.is_empty()));
    // The monitor's columns side by side, as at the bottom or merged into two, if
    // that fits (only with the parts below side by side too).
    let mut side_by_side = None;
    if let Some(mon) = mon.filter(|_| lower_fits || !has_lower) {
        for mon in [mon.clone(), two_columns(mon)] {
            let h = mon.columns.iter().map(Vec::len).max().unwrap_or(0) as f32 * MON_ROW + 4.0;
            let w = monitor_block(None, &g, pal, &mon, 0.0, h)?;
            if w + 8.0 <= width {
                side_by_side = Some((mon, w, h));
                break;
            }
        }
    }
    if let Some((mon, mon_w, mon_h)) = side_by_side {
        part(&mut y);
        if let Some(c) = c {
            c.set_offset(((width - mon_w) / 2.0).round(), y);
            monitor_block(Some(c), &g, pal, &mon, 0.0, mon_h)?;
            c.set_offset(0.0, 0.0);
        }
        y += mon_h;
    } else if let Some(mon) = mon {
        part(&mut y);
        let mut lines = Vec::new();
        let mut colors = Vec::new();
        for cell in mon.columns.iter().flatten() {
            lines.push((g.text(cell.label, MON_FONT, NORMAL, None)?, g.text(&cell.value, MON_FONT, SEMIBOLD, None)?));
            colors.push(match cell.level {
                Some(l) if l >= 90.0 => pal.level(f64::from(l)),
                _ => pal.text,
            });
        }
        let stack = stacked(&lines);
        for ((label, value), color) in lines.iter().zip(colors) {
            y = column_line(c, (left, right), y, MON_ROW, stack, (label, pal.text_dim), (value, color));
        }
    }

    // The session dots next to the limits, as at the bottom, when they fit.
    if lower_fits {
        part(&mut y);
        if let Some(c) = c {
            c.set_offset(((width - lower_w) / 2.0).round(), y - 4.0);
            render(Some(c), &lower, LINES_H)?;
            c.set_offset(0.0, 0.0);
        }
        y += LINES_H - 8.0;
        return Ok(y + if m.gpg_left.is_some() { 9.0 } else { 5.0 });
    }

    if m.sessions.iter().any(|&n| n > 0) {
        part(&mut y);
        for (status, n) in STATES.iter().zip(m.sessions) {
            if n == 0 {
                continue;
            }
            let count = g.text(&n.to_string(), STATE_FONT, SEMIBOLD, None)?;
            if let Some(c) = c {
                let w = 2.0 * DOT_R + 3.0 + count.width;
                let x = (width - w) / 2.0;
                let mid = y + STATE_ROW_H / 2.0;
                state_dot(c, pal, m, *status, x + DOT_R, mid);
                c.text(&count, x + 2.0 * DOT_R + 3.0, mid - count.height / 2.0, pal.text_dim);
            }
            y += STATE_ROW_H + 1.0;
        }
    }

    let limits = slim_limits(m);
    if !limits.is_empty() {
        part(&mut y);
        let marked = m.codex.is_some() || m.copilot.is_some();
        let mut lines = Vec::new();
        for (_, label, used, countdown, _) in &limits {
            let value = countdown.clone().unwrap_or_else(|| used.map_or_else(|| "–".to_string(), |u| format!("{u}%")));
            lines.push((g.text(label, FONT, NORMAL, None)?, g.text(&value, FONT, SEMIBOLD, None)?));
        }
        let stack = stacked(&lines);
        let mut last = None;
        for ((provider, label, used, _, pace), (l, v)) in limits.iter().zip(&lines) {
            if marked && last != Some(*provider) {
                let (symbol, color) = mark(*provider, pal);
                let t = g.text(symbol, MARK_FONT, SEMIBOLD, None)?;
                if let Some(c) = c {
                    c.text(&t, (width - t.width) / 2.0, y, color.with_alpha(fade));
                }
                y += t.height;
            }
            last = Some(*provider);
            let level = used.map(f64::from).unwrap_or(0.0);
            let color = limit_color(pal, label, level, *pace).with_alpha(fade);
            let value_color = if stands_out(label, level, *pace) { color } else { pal.text.with_alpha(fade) };
            y = column_line(c, (left, right), y, COL_ROW, stack, (l, pal.text_dim.with_alpha(fade)), (v, value_color));
        }
    }
    // Room for the GPG line along the bottom.
    Ok(y + if m.gpg_left.is_some() { 9.0 } else { 5.0 })
}

/// The monitor's values in two columns: all but the last column's in the first
/// (network and disk), the last one's (CPU, memory, GPU) in the second.
fn two_columns(mon: &Monitor) -> Monitor {
    let mut columns: Vec<Vec<Cell>> = mon.columns.iter().filter(|c| !c.is_empty()).cloned().collect();
    if columns.len() > 2 {
        let last = columns.pop().unwrap_or_default();
        columns = vec![columns.concat(), last];
    }
    Monitor { columns }
}

/// One line of the column widget from `y`, `row` high: the label at the left,
/// the value at the right; `stacked` (a taskbar too narrow for its part's
/// lines), the label above the value, each centred like the clock below.
/// Returns where the next line starts.
fn column_line(
    c: Option<&Canvas>,
    (left, right): (f32, f32),
    y: f32,
    row: f32,
    stacked: bool,
    (label, label_color): (&gfx::Text, Color),
    (value, value_color): (&gfx::Text, Color),
) -> f32 {
    if !stacked {
        if let Some(c) = c {
            let mid = y + row / 2.0;
            c.text(label, left, mid - label.height / 2.0, label_color);
            c.text(value, right - value.width, mid - value.height / 2.0, value_color);
        }
        return y + row;
    }
    let centre = (left + right) / 2.0;
    if let Some(c) = c {
        c.text(label, centre - label.width / 2.0, y, label_color);
        c.text(value, centre - value.width / 2.0, y + label.height, value_color);
    }
    y + label.height + value.height + 2.0
}

/// A session state's dot centred on (`cx`, `cy`), with its attention effects:
/// the needs-you dot glows, shakes and turns red; the done dot turns red while
/// a cache runs out.
fn state_dot(c: &Canvas, pal: &Palette, m: &Model, status: Status, cx: f32, cy: f32) {
    let mut cx = cx;
    let mut color = pal.status(status);
    if status == Status::Done && m.attention.done_expiring {
        color = pal.crit;
        if m.attention.done_glow > 0.0 {
            c.glow(cx, cy, DOT_R * 3.2, color.with_alpha(m.attention.done_glow));
        }
    }
    if status == Status::NeedsInput {
        let a = m.attention;
        if a.urgent {
            color = pal.crit;
        }
        cx += a.shake;
        if a.glow > 0.0 {
            c.glow(cx, cy, DOT_R * 3.2, color.with_alpha(a.glow));
        }
    }
    match status {
        Status::Working => c.stroke_circle(cx, cy, DOT_R - 0.6, 1.3, color),
        _ => c.fill_circle(cx, cy, DOT_R, color),
    }
}

/// How many limits the slim widget takes turns with.
pub fn slim_turns(m: &Model) -> usize {
    slim_limits(m).len()
}

/// The limits the slim widget takes turns with: provider, label, used
/// percent and countdown. A limit without a value (the 5-hour one on Ultra)
/// is left out.
/// One limit of the slim widget: whose, its label, used percent, the countdown
/// to show instead, and the week's pace (Claude's weekly limit).
type SlimLimit = (Provider, &'static str, Option<u8>, Option<String>, Option<Pace>);

fn slim_limits(m: &Model) -> Vec<SlimLimit> {
    let mut limits = Vec::new();
    if m.claude || (m.codex.is_none() && m.copilot.is_none()) {
        limits.push((Provider::Claude, "W", m.weekly, m.countdown[0].clone(), m.weekly_pace));
        if m.five_hour.is_some() {
            limits.push((Provider::Claude, "5h", m.five_hour, m.countdown[1].clone(), None));
        }
        if m.day.is_some() {
            limits.push((Provider::Claude, DAY_LABEL, m.day, None, day_pace(m)));
        }
    }
    if let Some(cx) = &m.codex {
        limits.push((Provider::Codex, "W", cx.weekly, cx.countdown[0].clone(), None));
        if cx.five_hour.is_some() {
            limits.push((Provider::Codex, "5h", cx.five_hour, cx.countdown[1].clone(), None));
        }
    }
    if let Some(cp) = &m.copilot {
        limits.push((Provider::Copilot, "M", cp.used, cp.countdown.clone(), None));
    }
    limits
}

/// The slim widget: the session dots in a row on top, below one limit at a
/// time (`turn` picks it). As wide as the widest limit, so turns don't move it.
fn slim(
    c: Option<&Canvas>,
    g: &gfx::Gfx,
    pal: &Palette,
    m: &Model,
    height: f32,
    row_mid: [f32; 2],
    turn: usize,
) -> windows::core::Result<f32> {
    let x = PAD_X;
    // Top: dot and count per state with sessions.
    let mut dots = Vec::new();
    for (status, n) in STATES.iter().zip(m.sessions) {
        if n > 0 {
            dots.push((*status, g.text(&n.to_string(), STATE_FONT, SEMIBOLD, None)?));
        }
    }
    let dots_w = dots.iter().map(|(_, t)| 2.0 * DOT_R + 3.0 + t.width).sum::<f32>()
        + DOT_GAP * dots.len().saturating_sub(1) as f32;

    // Bottom: one limit, measured against all of them.
    let limits = slim_limits(m);
    let marked = m.codex.is_some() || m.copilot.is_some();
    let value_of = |used: Option<u8>, countdown: &Option<String>| {
        countdown.clone().unwrap_or_else(|| used.map_or_else(|| "–".to_string(), |u| format!("{u}%")))
    };
    let (mut mark_w, mut label_w, mut value_w) = (0.0f32, 0.0f32, 0.0f32);
    for (provider, label, used, countdown, _) in &limits {
        if marked {
            mark_w = mark_w.max(g.text(mark(*provider, pal).0, MARK_FONT, SEMIBOLD, None)?.width + 3.0);
        }
        label_w = label_w.max(g.text(label, FONT, NORMAL, None)?.width);
        value_w = value_w.max(g.text(&value_of(*used, countdown), FONT, SEMIBOLD, None)?.width);
    }
    let bar_space = if m.bars { BAR_W + VALUE_GAP } else { 0.0 };
    let limit_w = if limits.is_empty() { 0.0 } else { mark_w + label_w + LABEL_GAP + bar_space + value_w };

    if let Some(c) = c {
        // Without sessions, the limit sits in the middle.
        let limit_mid = if dots.is_empty() { height / 2.0 } else { row_mid[1] };
        let mut dx = x;
        for (status, count) in &dots {
            state_dot(c, pal, m, *status, dx + DOT_R, row_mid[0]);
            c.text(count, dx + 2.0 * DOT_R + 3.0, row_mid[0] - count.height / 2.0, pal.text_dim);
            dx += 2.0 * DOT_R + 3.0 + count.width + DOT_GAP;
        }
        if let Some((provider, label, used, countdown, pace)) = limits.get(turn % limits.len().max(1)) {
            let fade = if m.stale { 0.5 } else { 1.0 };
            if marked {
                let (symbol, color) = mark(*provider, pal);
                let t = g.text(symbol, MARK_FONT, SEMIBOLD, None)?;
                c.text(&t, x, limit_mid - t.height / 2.0, color.with_alpha(fade));
            }
            let lx = x + mark_w;
            let l = g.text(label, FONT, NORMAL, None)?;
            c.text(&l, lx, limit_mid - l.height / 2.0, pal.text_dim.with_alpha(fade));
            let level = used.map(f64::from).unwrap_or(0.0);
            let color = limit_color(pal, label, level, *pace).with_alpha(fade);
            let bar_x = lx + label_w + LABEL_GAP;
            if m.bars {
                let bar_y = (limit_mid - BAR_H / 2.0).round();
                c.fill_round_rect(bar_x, bar_y, BAR_W, BAR_H, BAR_H / 2.0, pal.track);
                if let Some(u) = used.filter(|u| *u > 0) {
                    let fill = (BAR_W * f32::from(u.min(100)) / 100.0).max(BAR_H);
                    c.fill_round_rect(bar_x, bar_y, fill, BAR_H, BAR_H / 2.0, color);
                }
            }
            let v = g.text(&value_of(*used, countdown), FONT, SEMIBOLD, None)?;
            let value_color = if stands_out(label, level, *pace) { color } else { pal.text.with_alpha(fade) };
            c.text(&v, bar_x + bar_space + value_w - v.width, limit_mid - v.height / 2.0, value_color);
        }
    }
    Ok(x + dots_w.max(limit_w) + PAD_X)
}

/// The system monitor's columns, starting at `x`: a label and a value per
/// row, a faint graph of the last half minute behind. Returns where it ends.
fn monitor_block(
    c: Option<&Canvas>,
    g: &gfx::Gfx,
    pal: &Palette,
    mon: &Monitor,
    x: f32,
    height: f32,
) -> windows::core::Result<f32> {
    let mut cx = x;
    for column in mon.columns.iter().filter(|col| !col.is_empty()) {
        if cx > x {
            cx += MON_COL_GAP;
        }
        let mut label_w: f32 = 0.0;
        let mut value_w: f32 = 0.0;
        for cell in column {
            label_w = label_w.max(g.text(cell.label, MON_FONT, NORMAL, None)?.width);
            for w in cell.widest {
                value_w = value_w.max(g.text(w, MON_FONT, SEMIBOLD, None)?.width);
            }
        }
        let col_w = label_w + 4.0 + value_w;
        let col_top = ((height - column.len() as f32 * MON_ROW) / 2.0).round();
        if let Some(c) = c {
            for (i, cell) in column.iter().enumerate() {
                let y0 = col_top + i as f32 * MON_ROW;
                if cell.history.len() >= 2 {
                    let n = cell.history.len();
                    let step = col_w / (HISTORY - 1) as f32;
                    let pts: Vec<(f32, f32)> = cell
                        .history
                        .iter()
                        .enumerate()
                        .map(|(j, v)| {
                            let px = cx + step * (HISTORY.saturating_sub(n) + j) as f32;
                            (px, y0 + MON_ROW - 1.0 - v.clamp(0.0, 1.0) * (MON_ROW - 2.0))
                        })
                        .collect();
                    c.area(
                        &pts,
                        y0 + MON_ROW - 1.0,
                        y0 + 1.0,
                        pal.accent.with_alpha(0.32),
                        pal.accent.with_alpha(0.08),
                    );
                }
                let label = g.text(cell.label, MON_FONT, NORMAL, None)?;
                let value = g.text(&cell.value, MON_FONT, SEMIBOLD, None)?;
                let mid = y0 + MON_ROW / 2.0;
                c.text(&label, cx, mid - label.height / 2.0, pal.text_dim);
                let color = match cell.level {
                    Some(l) if l >= 90.0 => pal.level(f64::from(l)),
                    _ => pal.text,
                };
                c.text(&value, cx + col_w - value.width, mid - value.height / 2.0, color);
            }
        }
        cx += col_w;
    }
    Ok(cx)
}

/// One provider's two rows (label, bar and value; the plan's name where a
/// limit is missing), starting at `x`. Returns where it ends.
#[allow(clippy::too_many_arguments, reason = "the limits, their look and where they go")]
fn limits_block(
    c: Option<&Canvas>,
    g: &gfx::Gfx,
    pal: &Palette,
    m: &Model,
    x: f32,
    row_mid: [f32; 2],
    rows: &[Row; 2],
    also: Option<&Row>,
) -> windows::core::Result<f32> {
    let mut label_w: f32 = 0.0;
    let mut value_w: f32 = 0.0;
    let mut texts = Vec::new();
    // Another row that takes turns with these: measured, not drawn.
    let mut also_w: f32 = 0.0;
    if let Some(row) = also {
        match row {
            Row::Limit(label, used, countdown, _) => {
                let value =
                    countdown.clone().unwrap_or_else(|| used.map_or_else(|| "–".to_string(), |u| format!("{u}%")));
                label_w = label_w.max(g.text(label, FONT, NORMAL, None)?.width);
                value_w = value_w.max(g.text(&value, FONT, SEMIBOLD, None)?.width);
            }
            Row::Plan(name) => {
                also_w =
                    g.text(&crate::plan::label(name), FONT, SEMIBOLD, None)?.width + 2.0 * crate::plan::margin(name);
            }
            Row::Note(text) => also_w = g.text(text, FONT, NORMAL, None)?.width,
        }
    }
    for row in rows {
        texts.push(match row {
            Row::Limit(label, used, countdown, _) => {
                let l = g.text(label, FONT, NORMAL, None)?;
                let value =
                    countdown.clone().unwrap_or_else(|| used.map_or_else(|| "–".to_string(), |u| format!("{u}%")));
                let v = g.text(&value, FONT, SEMIBOLD, None)?;
                label_w = label_w.max(l.width);
                value_w = value_w.max(v.width);
                (l, Some(v))
            }
            Row::Plan(name) => (g.text(&crate::plan::label(name), FONT, SEMIBOLD, None)?, None),
            Row::Note(text) => (g.text(text, FONT, NORMAL, None)?, None),
        });
    }
    let bar_x = x + label_w + LABEL_GAP;
    let bar_space = if m.bars { BAR_W + VALUE_GAP } else { 0.0 };
    let value_right = bar_x + bar_space + value_w;
    let mut right = value_right.max(x + also_w);
    let fade = if m.stale { 0.5 } else { 1.0 };
    for (i, (row, (first, value))) in rows.iter().zip(&texts).enumerate() {
        let mid = row_mid[i];
        match (row, value) {
            (Row::Limit(label, used, _, pace), Some(value)) => {
                let Some(c) = c else { continue };
                c.text(first, x, mid - first.height / 2.0, pal.text_dim.with_alpha(fade));
                let level = used.map(f64::from).unwrap_or(0.0);
                let color = limit_color(pal, label, level, *pace).with_alpha(fade);
                if m.bars {
                    let bar_y = (mid - BAR_H / 2.0).round();
                    c.fill_round_rect(bar_x, bar_y, BAR_W, BAR_H, BAR_H / 2.0, pal.track);
                    if let Some(u) = used.filter(|u| *u > 0) {
                        let fill = (BAR_W * f32::from(u.min(100)) / 100.0).max(BAR_H);
                        c.fill_round_rect(bar_x, bar_y, fill, BAR_H, BAR_H / 2.0, color);
                    }
                }
                let value_color = if stands_out(label, level, *pace) { color } else { pal.text.with_alpha(fade) };
                c.text(value, value_right - value.width, mid - value.height / 2.0, value_color);
            }
            _ => {
                // Room for the plan's badge (Ultra) on both sides.
                let pad = if let Row::Plan(name) = row { crate::plan::margin(name) } else { 0.0 };
                right = right.max(x + first.width + 2.0 * pad);
                match (c, row) {
                    (Some(c), Row::Plan(name)) => {
                        crate::plan::draw(c, first, x + pad, mid - first.height / 2.0, name, pal, fade, m.shine);
                    }
                    (Some(c), _) => c.text(first, x, mid - first.height / 2.0, pal.text_dim.with_alpha(fade)),
                    _ => {}
                }
            }
        }
    }
    Ok(right)
}

/// Draws the hover highlight and content into the layered window, `w_px`
/// wide; content narrower than that (`content_px`, on the Widgets button) is
/// centred, with the background and highlight across the whole width.
fn present(hwnd: HWND, m: &Model, w_px: i32, content_px: i32, h_px: i32, dpi: u32) -> windows::core::Result<()> {
    let scale = dpi as f32 / 96.0;
    let (w, h) = (w_px as f32 / scale, h_px as f32 / scale);
    let shift = ((w_px - content_px).max(0) / 2) as f32 / scale;
    let pal = theme::palette(m.dark);
    // Inset from the taskbar's long edges: top and bottom, or (a column) left and right.
    let inset = |by: f32| if m.column || m.side { (by, 0.0, w - 2.0 * by, h) } else { (0.0, by, w, h - 2.0 * by) };
    gfx::present_layered(hwnd, w_px, h_px, dpi as f32, |c| {
        if let Some(bg) = m.background {
            let (x, y, w, h) = inset(3.0);
            c.fill_round_rect(x, y, w, h, 6.0, bg);
        }
        if m.hover {
            let (x, y, w, h) = inset(4.0);
            c.fill_round_rect(x, y, w, h, 4.0, pal.hover);
        }
        c.set_offset(shift, 0.0);
        let _ = if m.column { column(Some(c), m, w) } else { render(Some(c), m, h) };
        c.set_offset(0.0, 0.0);
        // How long the GPG signing key stays unlocked: a thin line along the bottom.
        if let Some(left) = m.gpg_left {
            let (x, w) = (PAD_X, w_px as f32 / scale - 2.0 * PAD_X);
            let y = h - 5.0;
            c.fill_round_rect(x, y, w, 2.0, 1.0, pal.track);
            let fill = (w * f32::from(left.fraction) / 100.0).max(2.0);
            c.fill_round_rect(x, y, fill, 2.0, 1.0, if left.ending { pal.warn } else { pal.accent });
        }
    })
}

/// The GPG key's remaining time, as shown in the widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpgLeft {
    /// Percent of the time left, 0 to 100 (whole steps, so redraws stay rare).
    pub fraction: u8,
    /// In the last minutes.
    pub ending: bool,
}

/// The widgets, one on each taskbar it's shown on.
pub struct Widgets {
    bars: Vec<Bar>,
    /// The taskbars last seen, and their displays (looked up when they change).
    seen: Vec<Taskbar>,
    displays: Vec<Display>,
    /// Button and clock positions per taskbar, from UI Automation.
    edges: Vec<uia::Edges>,
    hover: Option<HWND>,
    /// The widget clicked last: windows open next to it.
    active: Option<HWND>,
}

impl Widgets {
    pub const fn new() -> Widgets {
        Widgets {
            bars: Vec::new(),
            seen: Vec::new(),
            displays: Vec::new(),
            edges: Vec::new(),
            hover: None,
            active: None,
        }
    }

    /// Starts over after Explorer restarted. The windows usually died with
    /// the taskbars, but not always: one may already sit on the new taskbar,
    /// and Windows sends the same message when the main display's scaling
    /// changes. Those are destroyed, or they'd stay behind under the new ones.
    /// The positions from UI Automation stay: its thread only sends them again
    /// when they change.
    pub fn reset(&mut self) {
        for b in self.bars.drain(..) {
            b.destroy();
        }
        self.seen.clear();
        self.hover = None;
    }

    /// New positions from UI Automation; true if anything moved.
    pub fn set_edges(&mut self, edges: Vec<uia::Edges>) -> bool {
        let changed = self.edges != edges;
        self.edges = edges;
        changed
    }

    /// The mouse entered or left a widget; true if that changes a highlight.
    pub fn set_hover(&mut self, hwnd: HWND, on: bool) -> bool {
        let before = self.hover;
        if on {
            self.hover = Some(hwnd);
        } else if self.hover == Some(hwnd) {
            self.hover = None;
        }
        before != self.hover
    }

    /// The widget under the mouse.
    pub fn hovered(&self) -> Option<HWND> {
        self.hover
    }

    /// Screen rectangle and DPI of the widget `hwnd`.
    pub fn rect_of(&self, hwnd: HWND) -> Option<(RECT, u32)> {
        let b = self.bars.iter().find(|b| b.hwnd == hwnd)?;
        Some((b.screen_rect()?, b.dpi()))
    }

    /// Windows opened from now on appear next to this widget.
    pub fn set_active(&mut self, hwnd: HWND) {
        self.active = Some(hwnd);
    }

    pub fn is_active(&self, hwnd: HWND) -> bool {
        self.anchor_bar().is_some_and(|b| b.hwnd == hwnd)
    }

    pub fn hwnds(&self) -> Vec<HWND> {
        self.bars.iter().map(|b| b.hwnd).collect()
    }

    /// The displays that have a taskbar, main first.
    pub fn displays(&self) -> &[Display] {
        &self.displays
    }

    /// Whether a widget is shown on a taskbar other than the main one.
    /// UI Automation found Windows' Widgets button on a taskbar.
    pub fn widgets_button(&self) -> bool {
        self.edges.iter().any(|e| e.widgets.is_some())
    }

    pub fn beyond_main(&self) -> bool {
        self.bars.iter().any(|b| !b.main)
    }

    fn anchor_bar(&self) -> Option<&Bar> {
        let alive = |b: &&Bar| b.screen_rect().is_some();
        self.active
            .and_then(|a| self.bars.iter().find(|b| b.hwnd == a))
            .filter(alive)
            .or_else(|| self.bars.iter().filter(alive).find(|b| b.main))
            .or_else(|| self.bars.iter().find(alive))
    }

    /// Where windows open: the widget clicked last, else the main one.
    /// Its screen rectangle and DPI.
    pub fn anchor(&self) -> Option<(RECT, u32)> {
        let b = self.anchor_bar()?;
        Some((b.screen_rect()?, b.dpi()))
    }

    /// For a bubble: the widget on the display of the window in front, so
    /// it shows where the user is looking; else the usual anchor.
    pub fn anchor_in_front(&self) -> Option<(RECT, u32)> {
        // SAFETY: plain queries; a missing window gives a null handle.
        let monitor = unsafe { MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONEAREST) }.0 as isize;
        let taskbar = self.seen.iter().find(|t| t.monitor == monitor).map(|t| t.hwnd);
        let bar = self.bars.iter().find(|b| Some(b.taskbar) == taskbar);
        match bar.and_then(|b| Some((b.screen_rect()?, b.dpi()))) {
            Some(found) => Some(found),
            None => self.anchor(),
        }
    }

    /// Creates, moves and redraws the widgets, and removes those whose
    /// taskbar is gone or no longer chosen.
    /// Draws the widgets again with `model`, as they are: for animation
    /// frames, which change nothing but the picture. Which taskbars there are
    /// is looked at by [`Self::update`], every second.
    pub fn redraw(&mut self, model: &Model, opts: &Options) {
        let mut m = model.clone();
        for b in self.bars.iter_mut().filter(|b| b.alive()) {
            m.hover = self.hover == Some(b.hwnd);
            let edges = self.edges.iter().find(|e| e.taskbar == b.taskbar.0 as isize).copied();
            b.update(&m, opts, edges);
        }
    }

    pub fn update(&mut self, model: &Model, opts: &Options) {
        let now = taskbars::list();
        if now != self.seen {
            self.displays = taskbars::describe(&now);
            self.seen = now;
        }
        let wanted: Vec<HWND> =
            taskbars::pick(&self.displays, opts.taskbars, &opts.chosen).iter().map(|d| d.taskbar).collect();
        let (mut hover, mut active) = (self.hover, self.active);
        self.bars.retain_mut(|b| {
            let keep = wanted.contains(&b.taskbar) && b.alive();
            if !keep {
                // A destroyed window gets no "mouse left": forget it here.
                hover = hover.filter(|h| *h != b.hwnd);
                active = active.filter(|h| *h != b.hwnd);
                b.destroy();
            }
            keep
        });
        (self.hover, self.active) = (hover, active);
        for taskbar in &wanted {
            if !self.bars.iter().any(|b| b.taskbar == *taskbar) {
                let main = self.seen.iter().any(|t| t.hwnd == *taskbar && t.main);
                if let Some(bar) = Bar::create(*taskbar, main) {
                    self.bars.push(bar);
                }
            }
        }
        let mut m = model.clone();
        for b in &mut self.bars {
            m.hover = self.hover == Some(b.hwnd);
            let edges = self.edges.iter().find(|e| e.taskbar == b.taskbar.0 as isize).copied();
            b.update(&m, opts, edges);
        }
    }
}

/// The widget on one taskbar.
struct Bar {
    hwnd: HWND,
    taskbar: HWND,
    main: bool,
    /// Last drawn: model, height (a column: width), DPI, and the Widgets button's width it fills.
    drawn: Option<(Model, i32, u32, i32)>,
    /// x, y, width, height in taskbar client pixels.
    pos: (i32, i32, i32, i32),
    /// Taskbar colour and when it was read.
    sampled: Option<(std::time::Instant, Color)>,
    /// On a side taskbar: two lines last time, not a column.
    lines: bool,
}

impl Bar {
    fn create(taskbar: HWND, main: bool) -> Option<Bar> {
        // SAFETY: a child window of the (checked) taskbar with our registered class.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS,
                w!("Trayward"),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                Some(taskbar),
                None,
                None,
                None,
            )
        }
        .ok()?;
        Some(Bar { hwnd, taskbar, main, drawn: None, pos: (0, 0, 0, 0), sampled: None, lines: false })
    }

    fn alive(&self) -> bool {
        // SAFETY: plain query; a stale handle just says no.
        unsafe { IsWindow(Some(self.hwnd)).as_bool() }
    }

    fn destroy(&self) {
        if self.alive() {
            // SAFETY: our own window.
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }

    /// Screen rectangle of the widget, for placing windows next to it.
    fn screen_rect(&self) -> Option<RECT> {
        let mut r = RECT::default();
        // SAFETY: valid out-pointer; failure just yields None.
        unsafe { GetWindowRect(self.hwnd, &mut r) }.ok()?;
        // Not placed yet (waiting for the clock's position).
        (r.right - r.left > 1).then_some(r)
    }

    /// The taskbar's background colour, read from the screen at (`x`, `y`):
    /// just past the widget, near the taskbar's outer edge (where button
    /// highlights don't reach). Re-read now and then; the taskbar can be
    /// translucent.
    fn taskbar_color(&mut self, x: i32, y: i32) -> Color {
        if let Some((at, c)) = self.sampled {
            if at.elapsed() < std::time::Duration::from_secs(10) {
                return c;
            }
        }
        // SAFETY: screen DC obtained and released here.
        let c = unsafe {
            let dc = GetDC(None);
            let px = GetPixel(dc, x, y);
            ReleaseDC(None, dc);
            px
        };
        let color = if c.0 == CLR_INVALID {
            theme::palette(theme::system_dark()).surface
        } else {
            Color {
                r: (c.0 & 0xFF) as f32 / 255.0,
                g: ((c.0 >> 8) & 0xFF) as f32 / 255.0,
                b: ((c.0 >> 16) & 0xFF) as f32 / 255.0,
                a: 1.0,
            }
        };
        self.sampled = Some((std::time::Instant::now(), color));
        color
    }

    /// DPI the widget was last drawn at.
    fn dpi(&self) -> u32 {
        self.drawn.as_ref().map_or(96, |(_, _, dpi, _)| *dpi)
    }

    /// Keeps the widget left of the tray area (or the clock, on the taskbars
    /// of other displays), or over the Widgets button, and redraws it if
    /// `model` changed. `edges` says where the app buttons end, the clock
    /// starts and the Widgets button is, if known.
    fn update(&mut self, model: &Model, opts: &Options, edges: Option<uia::Edges>) {
        let (taskbar, hwnd) = (self.taskbar, self.hwnd);
        let buttons_right = edges.and_then(|e| e.buttons_right);
        // The Widgets button's place (screen pixels), when the widget goes there.
        let slot = edges.and_then(|e| e.widgets).filter(|_| opts.place == settings::Place::Widgets);
        // SAFETY: Win32 window calls on handles we check for validity first.
        unsafe {
            let mut tb = RECT::default();
            if GetWindowRect(taskbar, &mut tb).is_err() {
                return;
            }
            let dpi = GetDpiForWindow(taskbar).max(96);
            let scale = dpi as f32 / 96.0;
            if taskbars::edge(taskbar, &tb).vertical() {
                self.update_column(model, opts, edges, &tb, dpi);
                return;
            }
            let mut tray = RECT::default();
            let tray_ok = if self.main {
                FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), PCWSTR::null())
                    .is_ok_and(|t| GetWindowRect(t, &mut tray).is_ok())
            } else {
                // The clock, as found by UI Automation; wait for it before showing.
                let Some(e) = edges else { return };
                // Without a clock: left of the show-desktop strip at the far right.
                let left = e.clock_left.unwrap_or(tb.right - (12.0 * scale) as i32);
                tray = RECT { left, ..tb };
                true
            };
            if !tray_ok {
                tray = RECT { left: tb.right, ..tb };
            }
            let h = tb.bottom - tb.top;

            let gap = (2.0 * scale) as i32;
            let mut model = model.clone();
            model.bars = match opts.bars {
                settings::Bars::Always => true,
                settings::Bars::Never => false,
                settings::Bars::Auto => {
                    // Bars only if the widget then still fits before the app buttons
                    // (or into the Widgets button).
                    model.bars = true;
                    let full = render(None, &model, h as f32 / scale).map_or(0, |w| (w * scale).ceil() as i32);
                    match slot {
                        Some((left, right)) => full <= right - left,
                        None => buttons_right.is_none_or(|right| tray.left - gap - full >= right + gap),
                    }
                }
            };
            // On the Widgets button always: the weather underneath mustn't show through.
            let solid = opts.solid || slot.is_some();
            model.background = if solid { Some(self.taskbar_color(tray.left + 2, tb.bottom - 2)) } else { None };

            // Too wide for the Widgets button: first without the bars (even when they're
            // always wanted), then without the system monitor, and only then slim, as if
            // chosen in the settings (the slim widget leaves out the monitor too, and more).
            if let Some((left, right)) = slot {
                let need = |m: &Model| render(None, m, h as f32 / scale).map_or(0, |w| (w * scale).ceil() as i32);
                if model.slim.is_none() && model.bars && need(&model) > right - left {
                    model.bars = false;
                }
                if model.slim.is_none() && model.monitor.is_some() && need(&model) > right - left {
                    model.monitor = None;
                }
                if model.slim.is_none() && need(&model) > right - left {
                    model.slim = Some(model.turn % slim_turns(&model).max(1));
                    (model.shine, model.pinned, model.monitor) = (None, None, None);
                }
            }

            // On the Widgets button, at least as wide as it: the whole button is the widget's.
            let room = slot.map_or(0, |(left, right)| right - left);
            let mut width = self.pos.2;
            let key = (model.clone(), h, dpi, room);
            if self.drawn.as_ref() != Some(&key) {
                let Ok(w_dip) = render(None, &model, h as f32 / scale) else { return };
                let content = (w_dip * scale).ceil() as i32;
                width = content.max(room);
                if present(hwnd, &model, width, content, h, dpi).is_ok() {
                    self.drawn = Some(key);
                }
            }

            let x = match slot {
                Some((left, right)) => slot_x(left, right, width, tb.left, tb.right) - tb.left,
                None => (tray.left - tb.left - width - gap).max(0),
            };
            let pos = (x, 0, width, h);
            let on_top = GetWindow(taskbar, GW_CHILD).is_ok_and(|c| c == hwnd);
            if pos != self.pos || !on_top || !IsWindowVisible(hwnd).as_bool() {
                // Stay above the taskbar's own content.
                let _ = SetWindowPos(hwnd, Some(HWND_TOP), pos.0, pos.1, pos.2, pos.3, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                self.pos = pos;
            }
        }
    }
}

impl Bar {
    /// [`Self::update`] on a taskbar at the left or right: the widget as a
    /// column as wide as the taskbar, just above the tray area (or the clock,
    /// on other displays' taskbars).
    fn update_column(&mut self, model: &Model, opts: &Options, edges: Option<uia::Edges>, tb: &RECT, dpi: u32) {
        let (taskbar, hwnd) = (self.taskbar, self.hwnd);
        let scale = dpi as f32 / 96.0;
        // SAFETY: Win32 window calls on handles checked by the caller.
        unsafe {
            let mut tray = RECT::default();
            let tray_top = if self.main {
                FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), PCWSTR::null())
                    .ok()
                    .filter(|t| GetWindowRect(*t, &mut tray).is_ok())
                    .map(|_| tray.top)
            } else {
                // The clock, as found by UI Automation; wait for it before showing.
                let Some(e) = edges else { return };
                e.clock_top
            }
            .unwrap_or(tb.bottom - (12.0 * scale) as i32);
            let width = tb.right - tb.left;
            let gap = (2.0 * scale) as i32;

            let mut model = model.clone();
            (model.bars, model.slim, model.shine, model.pinned) = (false, None, None, None);
            model.background = if opts.solid { Some(self.taskbar_color(tb.right - 2, tray_top + 2)) } else { None };

            // A wide taskbar (one with labels) takes the widget as at the bottom, two
            // lines high, when it fits; else everything stacks in a column. Back to two
            // lines only with some room to spare, so a value that grows by a digit
            // doesn't make it jump between the two.
            model.side = true;
            let spare = if self.lines { 0.0 } else { LINES_SPARE };
            self.lines = lines_fit(&model, width as f32 / scale - spare);
            model.column = !self.lines;
            let mut height = self.pos.3;
            let key = (model.clone(), width, dpi, 0);
            if self.drawn.as_ref() != Some(&key) {
                let (content, h_dip) = if model.column {
                    (width, column(None, &model, width as f32 / scale))
                } else {
                    let w = render(None, &model, LINES_H).map_or(width, |w| (w * scale).ceil() as i32);
                    // The GPG line goes below the two lines, not through the second.
                    (w, Ok(LINES_H + if model.gpg_left.is_some() { 8.0 } else { 0.0 }))
                };
                let Ok(h_dip) = h_dip else { return };
                height = (h_dip * scale).ceil() as i32;
                if present(hwnd, &model, width, content, height, dpi).is_ok() {
                    self.drawn = Some(key);
                }
            }

            let y = (tray_top - tb.top - height - gap).max(0);
            let pos = (0, y, width, height);
            let on_top = GetWindow(taskbar, GW_CHILD).is_ok_and(|c| c == hwnd);
            if pos != self.pos || !on_top || !IsWindowVisible(hwnd).as_bool() {
                // Stay above the taskbar's own content.
                let _ = SetWindowPos(hwnd, Some(HWND_TOP), pos.0, pos.1, pos.2, pos.3, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                self.pos = pos;
            }
        }
    }
}

/// How high the widget is on a side taskbar wide enough for its two lines.
const LINES_H: f32 = 40.0;
/// The room a side taskbar's column needs to spare before it turns into two lines.
const LINES_SPARE: f32 = 12.0;

/// Whether the widget as at the bottom (two lines) fits a side taskbar `width`
/// DIPs wide, with room at both sides.
fn lines_fit(m: &Model, width: f32) -> bool {
    let m = Model { column: false, ..m.clone() };
    render(None, &m, LINES_H).is_ok_and(|w| w + 8.0 <= width)
}

/// Where a widget `width` wide goes on the Widgets button (`left` to `right`,
/// on a taskbar from `tb_left` to `tb_right`): centred on it, or, if wider,
/// from its edge towards the middle of the taskbar, so it doesn't run off the
/// end or into the tray.
fn slot_x(left: i32, right: i32, width: i32, tb_left: i32, tb_right: i32) -> i32 {
    let room = right - left;
    let x = if width <= room {
        left + (room - width) / 2
    } else if left + room / 2 < (tb_left + tb_right) / 2 {
        left
    } else {
        right - width
    };
    x.clamp(tb_left, (tb_right - width).max(tb_left))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacks_in_a_column_on_a_side_taskbar() {
        let codex = Limits { weekly: Some(12), five_hour: Some(100), countdown: [None, None] };
        let busy = Model { five_hour: Some(17), plan: None, codex: Some(codex), ..model() };
        let wide = column(None, &busy, 60.0).unwrap();
        // Every limit on a line of its own: more limits, a taller column.
        assert!(wide > column(None, &model(), 60.0).unwrap());
        // Too narrow for label and value side by side: stacked, so taller still.
        assert!(column(None, &busy, 26.0).unwrap() > wide);
        // The GPG line gets room at the bottom.
        let gpg = Model { gpg_left: Some(GpgLeft { fraction: 50, ending: false }), ..busy.clone() };
        assert!(column(None, &gpg, 60.0).unwrap() > wide);
    }

    #[test]
    fn a_wide_side_taskbar_takes_two_lines() {
        let cell =
            |label, value: &str| Cell { label, value: value.into(), widest: &["100%"], level: None, history: vec![] };
        let m = Model {
            monitor: Some(Monitor { columns: vec![vec![cell("CPU", "23%"), cell("RAM", "77.7 GB")]] }),
            sessions: [0, 4, 1],
            bars: false,
            shine: None,
            plan: None,
            ..model()
        };
        // A taskbar with labels (about 160 DIPs at 125 %): two lines.
        assert!(lines_fit(&m, 160.0));
        // Labels hidden (about 50): a column.
        assert!(!lines_fit(&m, 52.0));
    }

    #[test]
    fn a_wide_side_taskbar_puts_the_dots_next_to_the_limits() {
        let cell =
            |label, value: &str| Cell { label, value: value.into(), widest: &["100%"], level: None, history: vec![] };
        let net = vec![cell("↑", "31 KB/s"), cell("↓", "5 KB/s"), cell("R", "0 KB/s"), cell("W", "28 KB/s")];
        let cpu = vec![cell("CPU", "16%"), cell("RAM", "76.2 GB"), cell("GPU", "6.9 GB")];
        let all = Model {
            monitor: Some(Monitor { columns: vec![net.clone(), cpu.clone()] }),
            sessions: [0, 6, 0],
            bars: false,
            shine: None,
            plan: None,
            ..model()
        };
        // Too much for two lines at 160 DIPs...
        assert!(!lines_fit(&all, 160.0));
        // (Three columns of the monitor make two.)
        let r = vec![cell("R", "0 KB/s"), cell("W", "28 KB/s")];
        let three = Monitor { columns: vec![net.iter().take(2).cloned().collect(), r, cpu.clone()] };
        assert_eq!(two_columns(&three).columns.iter().map(Vec::len).collect::<Vec<_>>(), [4, 3]);
        // ...but shorter than a line per value: the monitor's columns side by side
        // (four lines), the dots next to the limits (two).
        let wide = column(None, &all, 160.0).unwrap();
        let narrow = column(None, &all, 52.0).unwrap();
        assert!(wide < 4.0 * MON_ROW + 2.0 * ROW_H + 30.0, "{wide}");
        assert!(narrow > wide, "{narrow} > {wide}");
    }

    #[test]
    fn stacks_a_whole_part_when_one_line_does_not_fit() {
        let cell = |label, value: &str| Cell { label, value: value.into(), widest: &[], level: None, history: vec![] };
        let mon = |cells: Vec<Cell>| Model { monitor: Some(Monitor { columns: vec![cells] }), ..model() };
        // A width where "CPU 9%" fits side by side but "RAM 71.6 GB" doesn't.
        let g = gfx::get().unwrap();
        let line = |l: &str, v: &str| {
            g.text(l, MON_FONT, NORMAL, None).unwrap().width + 3.0 + g.text(v, MON_FONT, SEMIBOLD, None).unwrap().width
        };
        let inner = (line("CPU", "9%") + line("RAM", "71.6 GB")) / 2.0;
        let width = inner / 0.8;
        let ram = column(None, &mon(vec![cell("RAM", "71.6 GB")]), width).unwrap();
        let both = column(None, &mon(vec![cell("CPU", "9%"), cell("RAM", "71.6 GB")]), width).unwrap();
        // CPU is stacked too (taller than a line side by side), like RAM.
        assert!(both - ram > MON_ROW + 1.0, "{both} - {ram}");
        // On a wide taskbar, both side by side.
        let wide = column(None, &mon(vec![cell("CPU", "9%"), cell("RAM", "71.6 GB")]), 200.0).unwrap();
        let wide_ram = column(None, &mon(vec![cell("RAM", "71.6 GB")]), 200.0).unwrap();
        assert!((wide - wide_ram - MON_ROW).abs() < 0.01);
    }

    #[test]
    fn sits_on_the_widgets_button() {
        // Centred on it when it fits.
        assert_eq!(slot_x(3065, 3255, 150, 0, 3840), 3085);
        // Wider: from the button towards the middle (button near the tray)...
        assert_eq!(slot_x(3065, 3255, 210, 0, 3840), 3045);
        // ...or from the left end (centred taskbar icons put it there).
        assert_eq!(slot_x(0, 190, 210, 0, 3840), 0);
        // Never off the taskbar.
        assert_eq!(slot_x(10, 50, 300, 0, 200), 0);
    }

    fn model() -> Model {
        Model {
            dark: true,
            hover: false,
            weekly: Some(46),
            five_hour: None,
            plan: Some("Ultra".into()),
            stale: false,
            sessions: [1, 2, 3],
            attention: Attention::default(),
            bars: true,
            gpg_left: None,
            background: None,
            shine: Some(0.5),
            countdown: [None, None],
            claude: true,
            codex: None,
            copilot: None,
            pinned: None,
            monitor: None,
            slim: None,
            turn: 0,
            column: false,
            side: false,
            day: None,
            day_turn: false,
            weekly_pace: None,
            day_over_pace: false,
        }
    }

    #[test]
    fn the_slim_widget_takes_turns_without_moving() {
        let max = Model { five_hour: Some(17), plan: Some("Max".into()), ..model() };
        // Weekly, then the 5-hour limit; Ultra has only the weekly one.
        assert_eq!(slim_limits(&max).iter().map(|l| l.1).collect::<Vec<_>>(), ["W", "5h"]);
        assert_eq!(slim_limits(&model()).iter().map(|l| l.1).collect::<Vec<_>>(), ["W"]);
        let a = render(None, &Model { slim: Some(0), ..max.clone() }, 40.0).unwrap();
        let b = render(None, &Model { slim: Some(1), ..max.clone() }, 40.0).unwrap();
        assert_eq!(a, b);
        // Narrower than the usual widget, and without the system monitor or a pinned session.
        let full = render(None, &max, 40.0).unwrap();
        assert!(a < full, "{a} vs {full}");
        let cell = Cell { label: "CPU", value: "3%".into(), widest: &["100%"], level: None, history: vec![] };
        let busy = Model { slim: Some(0), monitor: Some(Monitor { columns: vec![vec![cell]] }), ..max };
        assert_eq!(render(None, &busy, 40.0).unwrap(), a);
        // Codex too: its limits join the turns.
        let both = Model {
            slim: Some(0),
            codex: Some(Limits { weekly: Some(5), five_hour: Some(1), countdown: [None, None] }),
            ..model()
        };
        assert_eq!(slim_limits(&both).len(), 3);
    }

    #[test]
    fn todays_limit_takes_turns_without_moving() {
        // Ultra: the plan's name and today's limit take turns in the second row.
        for base in [model(), Model { five_hour: Some(17), plan: Some("Max".into()), ..model() }] {
            let usual = render(None, &Model { day: Some(115), ..base.clone() }, 40.0).unwrap();
            let day = render(None, &Model { day: Some(115), day_turn: true, ..base.clone() }, 40.0).unwrap();
            assert_eq!(usual, day);
            // Without turns, the widget is as it was.
            assert!(render(None, &base, 40.0).unwrap() <= usual);
        }
        // Slim: all three in turn (W, 5h, D), or W and D on Ultra.
        let max = Model { five_hour: Some(17), plan: Some("Max".into()), day: Some(40), ..model() };
        assert_eq!(slim_limits(&max).iter().map(|l| l.1).collect::<Vec<_>>(), ["W", "5h", DAY_LABEL]);
        let ultra = Model { day: Some(40), ..model() };
        assert_eq!(slim_limits(&ultra).iter().map(|l| l.1).collect::<Vec<_>>(), ["W", DAY_LABEL]);
        let widths: Vec<f32> =
            (0..3).map(|turn| render(None, &Model { slim: Some(turn), ..max.clone() }, 40.0).unwrap()).collect();
        assert!(widths.iter().all(|w| *w == widths[0]));
        // Over its budget, today's limit is yellow; the others go by their levels.
        let pal = theme::palette(true);
        assert_eq!(limit_color(pal, DAY_LABEL, 130.0, None), pal.warn);
        assert_eq!(limit_color(pal, DAY_LABEL, 90.0, None), pal.bar);
        // Within its budget, but over today's pace.
        assert_eq!(limit_color(pal, DAY_LABEL, 79.0, Some(Pace::Over)), pal.warn);
        assert!(stands_out(DAY_LABEL, 79.0, Some(Pace::Over)) && !stands_out(DAY_LABEL, 79.0, None));
        assert_eq!(limit_color(pal, "W", 90.0, None), pal.warn);
        // The weekly limit by its pace: 30 % can be too fast, 50 % fine.
        assert_eq!(limit_color(pal, "W", 30.0, Some(Pace::Over)), pal.warn);
        assert_eq!(limit_color(pal, "W", 30.0, Some(Pace::Out)), pal.crit);
        assert_eq!(limit_color(pal, "W", 50.0, Some(Pace::Under)), pal.bar);
        assert!(stands_out("W", 30.0, Some(Pace::Over)) && !stands_out("W", 50.0, Some(Pace::Under)));
    }

    #[test]
    fn the_system_monitor_keeps_its_width() {
        let plain = render(None, &model(), 40.0).unwrap();
        let cell = |label, value: &str, history: Vec<f32>| Cell {
            label,
            value: value.into(),
            widest: &["100%"],
            level: Some(5.0),
            history,
        };
        let mon = |cpu: &str| Monitor {
            columns: vec![vec![], vec![cell("CPU", cpu, vec![0.1, 0.5]), cell("RAM", "61%", vec![])]],
        };
        let a = render(None, &Model { monitor: Some(mon("3%")), ..model() }, 40.0).unwrap();
        let b = render(None, &Model { monitor: Some(mon("100%")), ..model() }, 40.0).unwrap();
        assert!(a > plain + 30.0, "{a} vs {plain}");
        assert_eq!(a, b);
        // Nothing switched on: nothing shown.
        let empty = Monitor { columns: vec![vec![], vec![]] };
        assert_eq!(render(None, &Model { monitor: Some(empty), ..model() }, 40.0).unwrap(), plain);
    }

    #[test]
    fn grows_with_what_it_shows() {
        let plain = render(None, &model(), 40.0).unwrap();
        assert!(plain > 60.0);
        // Codex adds a block, a countdown is wider than a percentage.
        let codex = Limits { weekly: Some(41), five_hour: Some(18), countdown: [None, None] };
        let both = render(None, &Model { codex: Some(codex.clone()), ..model() }, 40.0).unwrap();
        assert!(both > plain + 40.0, "{both} vs {plain}");
        let counting = Model { countdown: [Some("2d 7h".into()), None], ..model() };
        assert!(render(None, &counting, 40.0).unwrap() > plain);
        // A pinned session takes room too.
        let pinned =
            Pinned { name: "shop".into(), status: Status::Working, line: "Working · 3m".into(), urgent: false };
        assert!(render(None, &Model { pinned: Some(pinned), ..model() }, 40.0).unwrap() > plain + 40.0);
        // Codex alone still works, with its mark.
        let codex_only = Model { claude: false, codex: Some(codex), ..model() };
        assert!(render(None, &codex_only, 40.0).unwrap() > 40.0);
    }

    /// Lays `m` out on a taskbar 40 DIPs high and draws it into memory at 150 %:
    /// as wide as laid out, and something shows. Returns the width.
    fn drawn(m: &Model) -> f32 {
        let w = render(None, m, 40.0).unwrap();
        let mut again = None;
        let px = gfx::testing::offscreen((w * 1.5).ceil() as i32, 60, 144.0, |c| {
            again = Some(render(Some(c), m, 40.0).unwrap());
        })
        .unwrap();
        assert!(gfx::testing::drew(&px));
        assert_eq!(again, Some(w));
        w
    }

    /// [`drawn`] for the column on a side taskbar `width` DIPs wide. Returns the height.
    fn drawn_column(m: &Model, width: f32) -> f32 {
        let h = column(None, m, width).unwrap();
        let mut again = None;
        let px = gfx::testing::offscreen(width.ceil() as i32, h.ceil() as i32, 96.0, |c| {
            again = Some(column(Some(c), m, width).unwrap());
        })
        .unwrap();
        assert!(gfx::testing::drew(&px));
        assert_eq!(again, Some(h));
        h
    }

    fn cell(label: &'static str, value: &str, level: Option<f32>, history: Vec<f32>) -> Cell {
        Cell { label, value: value.into(), widest: &["100%", "999 MB/s"], level, history }
    }

    /// The system monitor with graphs, a value near its end and an empty column.
    fn monitor() -> Monitor {
        let graph: Vec<f32> = (0..HISTORY + 5).map(|i| (i % 7) as f32 / 6.0).collect();
        Monitor {
            columns: vec![
                vec![cell("↑", "31 KB/s", None, graph.clone()), cell("↓", "5 KB/s", None, vec![0.5])],
                vec![],
                vec![
                    cell("CPU", "96%", Some(96.0), graph.clone()),
                    cell("RAM", "81%", Some(81.0), graph.iter().take(3).copied().collect()),
                    cell("GPU", "4%", Some(4.0), vec![]),
                ],
            ],
        }
    }

    /// Models for everything the widget can show, one part at a time.
    fn every_state() -> Vec<Model> {
        let busy = Attention { urgent: true, glow: 0.6, shake: 1.5, done_expiring: true, done_glow: 0.4 };
        let quiet = Attention { done_expiring: true, ..Attention::default() };
        let codex = Limits { weekly: Some(0), five_hour: None, countdown: [None, Some("3h 2m".into())] };
        let copilot = Monthly { used: Some(97), countdown: None, note: "9 left".into() };
        let pinned =
            |status, urgent| Pinned {
                name: "a-long-project-name".into(), status, line: "Working · 3m".into(), urgent
            };
        let max = Model { plan: Some("Max".into()), five_hour: Some(17), ..model() };
        vec![
            model(),
            Model { attention: busy, stale: true, ..model() },
            Model { attention: quiet, sessions: [0, 1, 0], ..model() },
            Model { sessions: [0, 0, 0], weekly: None, plan: None, ..model() },
            Model {
                weekly: Some(97),
                five_hour: Some(85),
                countdown: [Some("2d 7h".into()), Some("1h 3m".into())],
                ..max.clone()
            },
            Model { five_hour: None, bars: false, ..max.clone() },
            Model { weekly: Some(0), five_hour: Some(100), weekly_pace: Some(Pace::Out), ..max.clone() },
            Model { weekly_pace: Some(Pace::Over), day: Some(130), day_turn: true, ..max.clone() },
            Model { weekly_pace: Some(Pace::Under), day: Some(40), day_over_pace: true, ..max.clone() },
            Model { day: Some(60), day_turn: true, ..model() },
            Model { day: Some(60), plan: None, five_hour: None, ..model() },
            Model { pinned: Some(pinned(Status::Working, false)), ..model() },
            Model { pinned: Some(pinned(Status::NeedsInput, true)), shine: None, ..model() },
            Model { monitor: Some(monitor()), ..model() },
            Model { codex: Some(codex.clone()), copilot: Some(copilot.clone()), stale: true, ..max.clone() },
            Model { claude: false, copilot: Some(copilot), ..model() },
            Model { claude: false, codex: Some(codex), ..model() },
        ]
    }

    #[test]
    fn draws_every_state_in_both_themes() {
        for dark in [true, false] {
            for m in every_state() {
                let m = Model { dark, ..m };
                assert!(drawn(&m) > 2.0 * PAD_X);
            }
        }
    }

    #[test]
    fn the_slim_widget_draws_each_turn() {
        for m in every_state() {
            for turn in 0..=slim_turns(&m) {
                for bars in [true, false] {
                    let slim = Model { slim: Some(turn), bars, ..m.clone() };
                    // As wide in every turn.
                    let first = Model { slim: Some(0), ..slim.clone() };
                    assert_eq!(drawn(&slim), render(None, &first, 40.0).unwrap());
                }
            }
        }
    }

    #[test]
    fn the_column_draws_on_every_side_taskbar() {
        for m in every_state() {
            for width in [26.0, 48.0, 60.0, 160.0, 240.0] {
                let side = Model { column: true, side: true, ..m.clone() };
                assert!(drawn_column(&side, width) > 10.0);
            }
        }
        // The system monitor as at the bottom, its columns merged into two, or a line per value.
        let mon = Model { monitor: Some(monitor()), gpg_left: Some(GpgLeft { fraction: 10, ending: true }), ..model() };
        let heights: Vec<f32> = [300.0, 150.0, 60.0].iter().map(|w| drawn_column(&mon, *w)).collect();
        assert!(heights[0] <= heights[1] && heights[1] < heights[2], "{heights:?}");
        // Only the system monitor.
        let alone = Model { sessions: [0, 0, 0], ..mon };
        drawn_column(&alone, 300.0);
        // Two columns stay as they are.
        let two =
            Monitor { columns: vec![vec![cell("CPU", "1%", None, vec![])], vec![cell("RAM", "2%", None, vec![])]] };
        assert_eq!(two_columns(&two), two);
    }

    /// Without a window to show it on, the widget is still drawn: its background,
    /// highlight and GPG line, at the bottom and in a column.
    #[test]
    fn presents_every_look_without_a_window() {
        let looks = [
            Model { background: Some(Color { r: 0.1, g: 0.1, b: 0.1, a: 1.0 }), hover: true, ..model() },
            Model { gpg_left: Some(GpgLeft { fraction: 0, ending: true }), ..model() },
            Model { gpg_left: Some(GpgLeft { fraction: 80, ending: false }), side: true, hover: true, ..model() },
            Model {
                column: true,
                side: true,
                hover: true,
                background: Some(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }),
                ..model()
            },
        ];
        for m in looks {
            assert!(present(HWND::default(), &m, 300, 200, 60, 144).is_err());
        }
    }

    #[test]
    fn keeps_track_of_its_widgets() {
        let mut w = Widgets::new();
        let a = HWND(std::ptr::dangling_mut());
        assert!(w.hovered().is_none() && w.hwnds().is_empty() && w.displays().is_empty());
        // The mouse on a widget and off it again; off another one changes nothing.
        assert!(w.set_hover(a, true));
        assert!(!w.set_hover(a, true));
        assert_eq!(w.hovered(), Some(a));
        assert!(!w.set_hover(HWND::default(), false));
        assert!(w.set_hover(a, false));
        // UI Automation's positions: only a change counts.
        let edges = uia::Edges {
            taskbar: 1,
            buttons_right: Some(900),
            clock_left: None,
            clock_top: None,
            widgets: Some((0, 190)),
        };
        assert!(w.set_edges(vec![edges]));
        assert!(!w.set_edges(vec![edges]));
        assert!(w.widgets_button());
        // Without widgets on a taskbar, windows have nothing to open next to.
        w.set_active(a);
        assert!(!w.is_active(a));
        assert!(w.anchor().is_none() && w.anchor_in_front().is_none() && w.rect_of(a).is_none());
        assert!(!w.beyond_main());
        // A widget whose window is gone: not drawn, not an anchor, dropped on a reset.
        w.bars.push(Bar {
            hwnd: HWND::default(),
            taskbar: HWND::default(),
            main: false,
            drawn: None,
            pos: (0, 0, 0, 0),
            sampled: None,
            lines: false,
        });
        assert!(w.beyond_main() && w.hwnds() == [HWND::default()]);
        let opts = Options {
            bars: settings::Bars::Auto,
            solid: false,
            place: settings::Place::Tray,
            taskbars: settings::Taskbars::Main,
            chosen: Vec::new(),
        };
        w.redraw(&model(), &opts);
        assert!(w.rect_of(HWND::default()).is_none() && w.anchor().is_none());
        assert_eq!(w.bars[0].dpi(), 96);
        w.set_hover(a, true);
        w.reset();
        assert!(w.hwnds().is_empty() && w.hovered().is_none());
    }

    /// A widget whose own window is gone, placed on the desktop window as if it
    /// were a taskbar: everything is worked out (and drawn into memory), but
    /// nothing can be shown or moved.
    #[test]
    fn places_itself_without_a_window_of_its_own() {
        use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
        // SAFETY: a plain query.
        let desktop = unsafe { GetDesktopWindow() };
        let mut rect = RECT::default();
        // SAFETY: valid out-pointer.
        let _ = unsafe { GetWindowRect(desktop, &mut rect) };
        let opts =
            |bars, solid, place| Options { bars, solid, place, taskbars: settings::Taskbars::Main, chosen: Vec::new() };
        let mid = (rect.left + rect.right) / 2;
        let edges = |widgets| uia::Edges {
            taskbar: desktop.0 as isize,
            buttons_right: Some(mid),
            clock_left: Some(rect.right - 200),
            clock_top: Some(rect.bottom - 200),
            widgets,
        };
        let wide = Model { monitor: Some(monitor()), five_hour: Some(30), plan: Some("Max".into()), ..model() };
        for main in [true, false] {
            for (o, e) in [
                (opts(settings::Bars::Auto, false, settings::Place::Tray), Some(edges(None))),
                (opts(settings::Bars::Always, true, settings::Place::Tray), None),
                (opts(settings::Bars::Never, false, settings::Place::Tray), Some(edges(None))),
                // On the Widgets button: too narrow for bars, then for the monitor, then slim.
                (opts(settings::Bars::Always, false, settings::Place::Widgets), Some(edges(Some((10, 400))))),
                (opts(settings::Bars::Auto, false, settings::Place::Widgets), Some(edges(Some((10, 60))))),
                (opts(settings::Bars::Always, false, settings::Place::Widgets), Some(edges(Some((10, 60))))),
            ] {
                let mut bar = Bar {
                    hwnd: HWND::default(),
                    taskbar: desktop,
                    main,
                    drawn: None,
                    pos: (0, 0, 0, 0),
                    sampled: None,
                    lines: false,
                };
                bar.update(&wide, &o, e);
                bar.update(&wide, &o, e);
                // Never shown, so not kept as drawn: the next update tries again.
                assert!(bar.drawn.is_none());
                assert!(!bar.alive() && bar.screen_rect().is_none());
            }
        }
        // The taskbar's colour is read once and kept for a while.
        let mut bar = Bar {
            hwnd: HWND::default(),
            taskbar: desktop,
            main: true,
            drawn: None,
            pos: (0, 0, 0, 0),
            sampled: None,
            lines: false,
        };
        let first = bar.taskbar_color(rect.left, rect.top);
        assert_eq!(bar.taskbar_color(rect.right - 1, rect.bottom - 1), first);
        // Read again after a while; not on any screen: the theme's surface.
        let long_ago = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(11));
        let mut off = Bar { sampled: long_ago.map(|at| (at, first)), ..bar };
        let away = off.taskbar_color(-100_000, -100_000);
        assert_eq!(away, theme::palette(theme::system_dark()).surface);
        // No taskbar at all: nothing to do.
        let mut gone = Bar { taskbar: HWND::default(), sampled: None, ..off };
        gone.update(&model(), &opts(settings::Bars::Auto, true, settings::Place::Tray), None);
        assert!(gone.drawn.is_none() && gone.pos == (0, 0, 0, 0));
    }

    /// The same on a taskbar at the left or right: a column on a narrow one,
    /// two lines on one with labels, with some room to spare before it turns back.
    #[test]
    fn places_itself_on_a_side_taskbar_without_a_window_of_its_own() {
        use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
        // SAFETY: a plain query.
        let desktop = unsafe { GetDesktopWindow() };
        let opts = |solid| Options {
            bars: settings::Bars::Auto,
            solid,
            place: settings::Place::Tray,
            taskbars: settings::Taskbars::Main,
            chosen: Vec::new(),
        };
        let edges = uia::Edges {
            taskbar: desktop.0 as isize,
            buttons_right: None,
            clock_left: None,
            clock_top: Some(900),
            widgets: None,
        };
        let m = Model { gpg_left: Some(GpgLeft { fraction: 40, ending: false }), plan: None, ..model() };
        // As wide as the two lines are there: no bars, no shine, nothing pinned.
        let laid = Model { side: true, bars: false, shine: None, pinned: None, ..m.clone() };
        let two_lines = render(None, &laid, LINES_H).unwrap() + 8.0;
        for main in [true, false] {
            for (dpi, solid) in [(96, false), (144, true)] {
                let scale = dpi as f32 / 96.0;
                let mut bar = Bar {
                    hwnd: HWND::default(),
                    taskbar: desktop,
                    main,
                    drawn: None,
                    pos: (0, 0, 0, 0),
                    sampled: None,
                    lines: false,
                };
                // Narrow: a column.
                let narrow = RECT { left: 0, top: 0, right: (48.0 * scale) as i32, bottom: 1000 };
                bar.update_column(&m, &opts(solid), Some(edges), &narrow, dpi);
                assert!(!bar.lines && bar.drawn.is_none());
                // Just wide enough for two lines, but without room to spare: still a column.
                let just = RECT { right: ((two_lines + 10.0) * scale).ceil() as i32, ..narrow };
                bar.update_column(&m, &opts(solid), Some(edges), &just, dpi);
                assert!(!bar.lines);
                // With labels: two lines, kept when it gets a little narrower again.
                let wide = RECT { right: (240.0 * scale) as i32, ..narrow };
                bar.update_column(&m, &opts(solid), Some(edges), &wide, dpi);
                assert!(bar.lines);
                bar.update_column(&m, &opts(solid), Some(edges), &just, dpi);
                assert!(bar.lines);
                // Another display's taskbar waits for its clock.
                let mut other = Bar { main: false, pos: (1, 2, 3, 4), ..bar };
                other.update_column(&m, &opts(solid), None, &wide, dpi);
                assert_eq!(other.pos, (1, 2, 3, 4));
            }
        }
    }

    /// Without the window classes (the app registers them when it starts), no
    /// widget window can be made: the taskbars are looked at, nothing is added to them.
    #[test]
    fn makes_no_window_without_its_class() {
        assert!(Bar::create(HWND::default(), true).is_none());
        let mut w = Widgets::new();
        let opts = Options {
            bars: settings::Bars::Auto,
            solid: false,
            place: settings::Place::Tray,
            taskbars: settings::Taskbars::All,
            chosen: Vec::new(),
        };
        w.update(&model(), &opts);
        w.update(&model(), &opts);
        assert!(w.hwnds().is_empty());
        assert_eq!(w.displays().len(), taskbars::list().len());
    }
}
