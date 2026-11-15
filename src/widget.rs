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
    /// Show the small usage bars (set by [`Widget::update`] from the options).
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
    /// Today's limit: what today used, as a percentage of its share of the
    /// week (over 100: more than its share). Set while limits take turns.
    pub day: Option<u8>,
    /// The second row shows today's limit now, in its turn with the 5-hour
    /// limit (or the plan's name).
    pub day_turn: bool,
    /// How the week's pace stands, for the weekly limit's colour.
    pub weekly_pace: Option<Pace>,
}

/// The label of today's limit.
pub const DAY_LABEL: &str = "D";

/// A limit's colour: the weekly and 5-hour limits turn yellow at 80 % and
/// red at 95 %; today's limit is a share of a day's budget, yellow once it's
/// over 100 %. Claude's weekly limit goes by the week's pace (which includes
/// the 80 and 95 %): yellow over an even pace, red when it runs out before the reset.
fn limit_color(pal: &Palette, label: &str, level: f64, pace: Option<Pace>) -> Color {
    match (label, pace) {
        (DAY_LABEL, _) if level > 100.0 => pal.warn,
        (DAY_LABEL, _) => pal.bar,
        (_, Some(Pace::Out)) => pal.crit,
        (_, Some(Pace::Over)) => pal.warn,
        (_, Some(Pace::Under)) => pal.bar,
        (_, None) => pal.level(level),
    }
}

/// Whether a limit's value stands out (in its colour rather than the text's).
fn stands_out(label: &str, level: f64, pace: Option<Pace>) -> bool {
    match (label, pace) {
        (DAY_LABEL, _) => level > 100.0,
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
        let day = m.day.map(|d| Row::Limit(DAY_LABEL, Some(d), None, None));
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
            limits.push((Provider::Claude, DAY_LABEL, m.day, None, None));
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
#[allow(clippy::too_many_arguments)]
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

/// Draws the hover highlight and content into the layered window.
fn present(hwnd: HWND, m: &Model, w_px: i32, h_px: i32, dpi: u32) -> windows::core::Result<()> {
    let scale = dpi as f32 / 96.0;
    let h = h_px as f32 / scale;
    let pal = theme::palette(m.dark);
    gfx::present_layered(hwnd, w_px, h_px, dpi as f32, |c| {
        if let Some(bg) = m.background {
            c.fill_round_rect(0.0, 3.0, w_px as f32 / scale, h - 6.0, 6.0, bg);
        }
        if m.hover {
            c.fill_round_rect(0.0, 4.0, w_px as f32 / scale, h - 8.0, 4.0, pal.hover);
        }
        let _ = render(Some(c), m, h);
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
    /// Last drawn: model, height and DPI.
    drawn: Option<(Model, i32, u32)>,
    /// x, y, width, height in taskbar client pixels.
    pos: (i32, i32, i32, i32),
    /// Taskbar colour and when it was read.
    sampled: Option<(std::time::Instant, Color)>,
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
        Some(Bar { hwnd, taskbar, main, drawn: None, pos: (0, 0, 0, 0), sampled: None })
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

    /// The taskbar's background colour, read from the screen just right of
    /// the widget near the bottom edge (where button highlights don't reach).
    /// Re-read now and then; the taskbar can be translucent.
    fn taskbar_color(&mut self, tb: &RECT, tray: &RECT) -> Color {
        if let Some((at, c)) = self.sampled {
            if at.elapsed() < std::time::Duration::from_secs(10) {
                return c;
            }
        }
        // SAFETY: screen DC obtained and released here.
        let c = unsafe {
            let dc = GetDC(None);
            let px = GetPixel(dc, tray.left + 2, tb.bottom - 2);
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
        self.drawn.as_ref().map_or(96, |(_, _, dpi)| *dpi)
    }

    /// Keeps the widget left of the tray area (or the clock, on the taskbars
    /// of other displays) and redraws it if `model` changed. `edges` says
    /// where the app buttons end and the clock starts, if known.
    fn update(&mut self, model: &Model, opts: &Options, edges: Option<uia::Edges>) {
        let (taskbar, hwnd) = (self.taskbar, self.hwnd);
        let buttons_right = edges.and_then(|e| e.buttons_right);
        // SAFETY: Win32 window calls on handles we check for validity first.
        unsafe {
            let mut tb = RECT::default();
            if GetWindowRect(taskbar, &mut tb).is_err() {
                return;
            }
            let dpi = GetDpiForWindow(taskbar).max(96);
            let scale = dpi as f32 / 96.0;
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
                    // Bars only if the widget then still fits before the app buttons.
                    model.bars = true;
                    let full = render(None, &model, h as f32 / scale).map_or(0, |w| (w * scale).ceil() as i32);
                    buttons_right.is_none_or(|right| tray.left - gap - full >= right + gap)
                }
            };
            model.background = if opts.solid { Some(self.taskbar_color(&tb, &tray)) } else { None };

            let mut width = self.pos.2;
            let key = (model.clone(), h, dpi);
            if self.drawn.as_ref() != Some(&key) {
                let Ok(w_dip) = render(None, &model, h as f32 / scale) else { return };
                width = (w_dip * scale).ceil() as i32;
                if present(hwnd, &model, width, h, dpi).is_ok() {
                    self.drawn = Some(key);
                }
            }

            let x = (tray.left - tb.left - width - gap).max(0);
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

#[cfg(test)]
mod tests {
    use super::*;

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
            day: None,
            day_turn: false,
            weekly_pace: None,
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
}
