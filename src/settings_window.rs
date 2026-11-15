//! The settings window, in the style of Windows 11 Settings: pages on the
//! left, each setting a card on the right with a switch or a set of choices.

use crate::gfx::{self, Canvas, NORMAL, SEMIBOLD};
use crate::i18n::{f, t, tr};
use crate::settings::{self, Bars, Prefs, SoundChoice, Taskbars, Volume};
use crate::sound::{self, Kind};
use crate::theme::{self, Color, Palette};
use crate::tracking;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    PAINTSTRUCT,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, IsWindowVisible, SW_HIDE, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
    SetForegroundWindow, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CAPTION, WS_MINIMIZEBOX,
    WS_OVERLAPPED, WS_SYSMENU,
};
use windows::core::{BOOL, PCWSTR, Result, w};

pub const CLASS: PCWSTR = w!("Trayward.Settings");

const WIDTH: f32 = 700.0;
const HEIGHT: f32 = 500.0;
const NAV_W: f32 = 190.0;
const PAD: f32 = 24.0;
const CARD_H: f32 = 66.0;
const SOUND_CARD_H: f32 = 108.0;
const GAP: f32 = 6.0;
const DISPLAY_CARD_H: f32 = 52.0;
/// Cards on the pages with the most of them (Taskbar, Limits).
const COMPACT_CARD_H: f32 = 56.0;

const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

/// Pages: name and Segoe Fluent Icons glyph.
pub const PAGES: [(&str, char); 11] = [
    ("General", '\u{E713}'),
    ("Taskbar", '\u{E770}'),
    ("System monitor", '\u{E9D9}'),
    ("Sessions", '\u{E8F2}'),
    ("Notifications", '\u{EA8F}'),
    ("Limits", '\u{E7BA}'),
    ("Providers", '\u{E774}'),
    ("Away", '\u{E8EA}'),
    ("MQTT", '\u{E80F}'),
    ("Mailbox", '\u{E715}'),
    ("About", '\u{E946}'),
];

/// Who made Trayward, and where.
const AUTHOR: &str = "Alex 'Delta' Diller";
const AUTHOR_URL: &str = "https://github.com/TheDelta";

const PLAY: char = '\u{E768}';
const FOLDER: char = '\u{E8B7}';

/// What the window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub dark: bool,
    pub prefs: Prefs,
    pub interval: u32,
    pub bars: Bars,
    pub solid: bool,
    pub taskbars: Taskbars,
    pub chosen: Vec<String>,
    /// Displays with a taskbar: id, name and where it is.
    pub displays: Vec<(String, String, String)>,
    /// More monitors than taskbars: Windows shows the taskbar on the main display only.
    pub more_monitors: bool,
    /// Codex is used on this PC (its folder exists).
    pub codex_found: bool,
    /// Copilot's agent is used on this PC (its folder exists).
    pub copilot_found: bool,
    /// Why Copilot's limit couldn't be read, if so.
    pub copilot_error: Option<String>,
    pub tracking: tracking::State,
    pub autostart: bool,
    /// How the last test push went, or why a paste didn't take.
    pub push_status: Option<String>,
    /// The broker and whether it's connected, or why a paste didn't take.
    pub mqtt_line: Option<String>,
    /// A newer release, if there is one.
    pub update: Option<crate::update::Release>,
    /// What the update check says: "Up to date", "Checking…", an error.
    pub update_status: String,
    /// How the mailbox's folder stands.
    pub mailbox_folder: MailboxFolder,
    /// Why a pasted mailbox folder wasn't taken, if so.
    pub mailbox_note: Option<String>,
    /// The mailbox's inboxes.
    pub mailbox_inboxes: Vec<InboxRow>,
}

/// How the mailbox's folder stands, for its card.
#[derive(Clone, Debug, PartialEq)]
pub enum MailboxFolder {
    /// Being looked for; the spinner's angle.
    Searching(u16),
    /// In use: set by hand, or found.
    Found { path: String, set: bool },
    /// Not there: the folder set by hand, or none was found.
    Missing(Option<String>),
}

/// One inbox of the mailbox, as the settings list it.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxRow {
    pub name: String,
    pub unread: usize,
    /// How long the oldest unread message has waited, e.g. "14h".
    pub waiting: Option<String>,
    pub done: usize,
    /// How long ago the last one was handled, e.g. "2h".
    pub last_done: Option<String>,
    /// A session runs in the project folder of this name.
    pub session: bool,
}

/// How a card's description is drawn when it says how something stands.
#[derive(Clone, Copy)]
struct DescStyle {
    color: Color,
    /// A turning arrow in front of it (the angle): something is being looked for.
    spin: Option<u16>,
}

/// What a click does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Page(usize),
    Autostart,
    Interval(u32),
    Animations,
    Mailbox,
    PasteMailbox,
    FindMailbox,
    /// Display mode: slim or normal.
    Slim(bool),
    /// How long each limit is shown when they take turns (0: no turns).
    TurnSecs(u8),
    DailyBudget(crate::forecast::Budget),
    Bars(Bars),
    Solid,
    Taskbars(Taskbars),
    /// Show or hide the widget on a display's taskbar (by id).
    Display(String),
    Hooks,
    Transcripts,
    Wsl,
    HideArchived,
    GpgTimer,
    SimulateFiveHour,
    Poll(u32),
    Bubbles,
    /// Sound for a kind: 0 off, 1 to 3 built-in, 4 custom file.
    Sound(Kind, u8),
    Play(Kind),
    ChooseFile(Kind),
    Volume(Volume),
    /// Warning levels in percent (0: off).
    WarnWeekly(u8),
    WarnFiveHour(u8),
    WarnPace,
    CountdownAt(u8),
    NotifyFree,
    Codex,
    /// Copilot: its sessions, and its monthly limit.
    Copilot(bool, bool),
    Shows(settings::Shown),
    Language(crate::i18n::Choice),
    Forge,
    ForgeBubbles,
    WeekSummary,
    Away,
    AwayAfter(u32),
    AwayDone,
    /// Take the push address from the clipboard.
    PasteUrl,
    TestPush,
    UpdateCheck,
    CheckNow,
    KeepStats,
    Mqtt,
    /// Take the broker's address from the clipboard.
    PasteBroker,
    TestMqtt,
    /// Open a web page.
    Open(String),
    /// Switch the system monitor as a whole on or off.
    MonitorOn,
    /// Switch a value of the system monitor (a `sysmon` bit) on or off.
    Monitor(u8),
    /// Show a value of the system monitor (a `sysmon` bit): off (`None`), as
    /// a percentage (`Some(false)`) or the other way (`Some(true)`: GB, rates).
    MonitorShow(u8, Option<bool>),
    MonitorGraphs,
}

type Hit = crate::hits::Hit<Action>;

struct Layout<'a> {
    c: Option<&'a Canvas<'a>>,
    g: std::rc::Rc<gfx::Gfx>,
    pal: &'static Palette,
    hits: Vec<Hit>,
    /// Hovered hit index from the previous layout.
    hover: Option<usize>,
    /// Cards go into this column (x, width) instead of the page's whole width.
    column: Option<(f32, f32)>,
    /// The next card's description, drawn this way (taken by `card`).
    desc_style: Option<DescStyle>,
}

impl Layout<'_> {
    fn next_hovered(&self) -> bool {
        self.hover == Some(self.hits.len())
    }

    fn push(&mut self, x: f32, y: f32, w: f32, h: f32, action: Action) {
        self.hits.push(Hit { x, y, w, h, action, tag: () });
    }

    /// A card with a title and a description, leaving `control_w` on the
    /// right for its control; returns its content box.
    fn card(
        &mut self,
        y: f32,
        h: f32,
        title: &str,
        desc: &str,
        hovered: bool,
        control_w: f32,
    ) -> Result<(f32, f32, f32)> {
        let (x, w) = self.column.unwrap_or((NAV_W + PAD, WIDTH - NAV_W - 2.0 * PAD));
        let text_w = w - 36.0 - control_w;
        let title = self.g.text(&tr(title), 14.0, NORMAL, Some(text_w))?;
        let style = self.desc_style.take();
        let spin =
            style.and_then(|s| s.spin).map(|a| self.g.text("↻", 12.0, NORMAL, None).map(|t| (t, a))).transpose()?;
        let indent = spin.as_ref().map_or(0.0, |(t, _)| t.width + 5.0);
        let desc =
            (!desc.is_empty()).then(|| self.g.text(&tr(desc), 12.0, NORMAL, Some(text_w - indent))).transpose()?;
        if let Some(c) = self.c {
            c.fill_round_rect(x, y, w, h, 6.0, if hovered { self.pal.card_hover } else { self.pal.card });
            let block = title.height + desc.as_ref().map_or(0.0, |d| d.height + 1.0);
            let top = if h > CARD_H { y + 14.0 } else { y + (h - block) / 2.0 };
            c.text(&title, x + 18.0, top, self.pal.text);
            if let Some(d) = &desc {
                let color = style.map_or(self.pal.text_dim, |s| s.color);
                let dy = top + title.height + 1.0;
                if let Some((arrow, angle)) = &spin {
                    c.text_rotated(arrow, x + 18.0, dy + (d.height - arrow.height) / 2.0, color, f32::from(*angle));
                }
                c.text(d, x + 18.0 + indent, dy, color);
            }
        }
        Ok((x, y, w))
    }

    /// A card whose whole area toggles a switch.
    fn toggle_card(&mut self, y: f32, title: &str, desc: &str, on: bool, action: Action) -> Result<f32> {
        self.toggle_card_h(y, CARD_H, title, desc, on, action)
    }

    /// A toggle card `h` high.
    fn toggle_card_h(&mut self, y: f32, h: f32, title: &str, desc: &str, on: bool, action: Action) -> Result<f32> {
        let hovered = self.next_hovered();
        // Room for the switch and its On/Off label.
        let (x, y, w) = self.card(y, h, title, desc, hovered, 80.0)?;
        let label = self.g.text(if on { t("On") } else { t("Off") }, 13.0, NORMAL, None)?;
        if let Some(c) = self.c {
            let (tw, th) = (40.0, 20.0);
            let tx = x + w - 18.0 - tw;
            let ty = y + (h - th) / 2.0;
            if on {
                c.fill_round_rect(tx, ty, tw, th, th / 2.0, self.pal.accent);
                c.fill_circle(tx + tw - th / 2.0, ty + th / 2.0, 6.0, self.pal.on_accent);
            } else {
                c.stroke_round_rect(tx + 0.5, ty + 0.5, tw - 1.0, th - 1.0, th / 2.0, 1.0, self.pal.stroke);
                c.fill_circle(tx + th / 2.0, ty + th / 2.0, 5.0, self.pal.stroke);
            }
            c.text(&label, tx - 10.0 - label.width, y + (h - label.height) / 2.0, self.pal.text);
        }
        self.push(x, y, w, h, action);
        Ok(h + GAP)
    }

    /// Segmented choices, right-aligned at `right` (or left-aligned at `left`),
    /// centred on `cy`. Returns the width used.
    fn segments(&mut self, left: Option<f32>, right: f32, cy: f32, options: &[(String, Action, bool)]) -> Result<f32> {
        let texts =
            options.iter().map(|(o, _, _)| self.g.text(&tr(o), 13.0, NORMAL, None)).collect::<Result<Vec<_>>>()?;
        let widths: Vec<f32> = texts.iter().map(|t| t.width + 22.0).collect();
        let total: f32 = widths.iter().sum();
        let h = 30.0;
        let mut x = left.unwrap_or(right - total);
        let y = cy - h / 2.0;
        if let Some(c) = self.c {
            c.stroke_round_rect(x + 0.5, y + 0.5, total - 1.0, h - 1.0, 5.0, 1.0, self.pal.divider.with_alpha(2.5));
        }
        for ((t, w), (_, action, selected)) in texts.iter().zip(&widths).zip(options) {
            let hovered = self.next_hovered();
            if let Some(c) = self.c {
                if *selected {
                    c.fill_round_rect(x + 2.0, y + 2.0, w - 4.0, h - 4.0, 4.0, self.pal.accent);
                } else if hovered {
                    c.fill_round_rect(x + 2.0, y + 2.0, w - 4.0, h - 4.0, 4.0, self.pal.row_hover);
                }
                let color = if *selected { self.pal.on_accent } else { self.pal.text };
                c.text(t, x + (w - t.width) / 2.0, cy - t.height / 2.0, color);
            }
            self.push(x, y, *w, h, action.clone());
            x += w;
        }
        Ok(total)
    }

    /// A card with segmented choices on the right.
    fn choice_card(&mut self, y: f32, title: &str, desc: &str, options: &[(String, Action, bool)]) -> Result<f32> {
        self.choice_card_h(y, CARD_H, title, desc, options)
    }

    /// A choice card `h` high.
    fn choice_card_h(
        &mut self,
        y: f32,
        h: f32,
        title: &str,
        desc: &str,
        options: &[(String, Action, bool)],
    ) -> Result<f32> {
        let mut control_w = 0.0;
        for (o, _, _) in options {
            control_w += self.g.text(&tr(o), 13.0, NORMAL, None)?.width + 22.0;
        }
        let (x, y, w) = self.card(y, h, title, desc, false, control_w)?;
        self.segments(None, x + w - 18.0, y + h / 2.0, options)?;
        Ok(h + GAP)
    }

    /// A small round icon button.
    fn icon_button(&mut self, cx: f32, cy: f32, glyph: char, action: Action) -> Result<()> {
        let hovered = self.next_hovered();
        let t = self.g.glyph(glyph, 14.0)?;
        let r = 15.0;
        if let Some(c) = self.c {
            if hovered {
                c.fill_circle(cx, cy, r, self.pal.row_hover);
            }
            c.stroke_circle(cx, cy, r - 0.5, 1.0, self.pal.divider.with_alpha(2.5));
            c.text(&t, cx - t.width / 2.0, cy - t.height / 2.0, self.pal.text);
        }
        self.push(cx - r, cy - r, 2.0 * r, 2.0 * r, action);
        Ok(())
    }

    fn sound_card(&mut self, y: f32, title: &str, kind: Kind, choice: &SoundChoice) -> Result<f32> {
        let desc = match choice {
            SoundChoice::Off => "No sound".to_string(),
            SoundChoice::Builtin(n) => sound::descriptions(kind)[usize::from(n.saturating_sub(1)).min(2)].to_string(),
            SoundChoice::File(path) => path.rsplit(['\\', '/']).next().unwrap_or(path).to_string(),
        };
        // The choices sit on their own line, so the text can use the full width.
        let (x, y, w) = self.card(y, SOUND_CARD_H, title, &desc, false, 0.0)?;
        let mut options = vec![("Off".to_string(), Action::Sound(kind, 0), *choice == SoundChoice::Off)];
        for (i, name) in sound::names(kind).iter().enumerate() {
            let n = i as u8 + 1;
            options.push((name.to_string(), Action::Sound(kind, n), *choice == SoundChoice::Builtin(n)));
        }
        options.push(("Custom".into(), Action::Sound(kind, 4), matches!(choice, SoundChoice::File(_))));
        let cy = y + SOUND_CARD_H - 30.0;
        let used = self.segments(Some(x + 18.0), 0.0, cy, &options)?;
        let mut bx = x + 18.0 + used + 26.0;
        if *choice != SoundChoice::Off {
            self.icon_button(bx, cy, PLAY, Action::Play(kind))?;
            bx += 38.0;
        }
        if matches!(choice, SoundChoice::File(_)) {
            self.icon_button(bx, cy, FOLDER, Action::ChooseFile(kind))?;
        }
        let _ = w;
        Ok(SOUND_CARD_H + GAP)
    }
}

/// Lays out the window (and draws it, with a canvas). Returns the clickable areas.
fn render<'a>(c: Option<&'a Canvas<'a>>, m: &Model, page: usize, hover: Option<usize>) -> Result<Vec<Hit>> {
    let pal = theme::palette(m.dark);
    let mut l = Layout { c, g: gfx::get()?, pal, hits: Vec::new(), hover, column: None, desc_style: None };

    // Navigation.
    let mut y = PAD + 8.0;
    if let Some(c) = c {
        let t = l.g.text("Trayward", 14.0, SEMIBOLD, None)?;
        c.text(&t, PAD, PAD - 4.0, pal.text);
        let v = l.g.text(env!("CARGO_PKG_VERSION"), 12.0, NORMAL, None)?;
        c.text(&v, PAD + t.width + 8.0, PAD - 4.0 + (t.height - v.height) / 2.0 + 1.0, pal.text_dim);
    }
    y += 24.0;
    for (i, (name, glyph)) in PAGES.iter().enumerate() {
        let hovered = l.next_hovered();
        let (x, w, h) = (PAD / 2.0, NAV_W - PAD, 36.0);
        if let Some(c) = c {
            if i == page {
                c.fill_round_rect(x, y, w, h, 5.0, pal.row_hover.with_alpha(1.6));
                c.fill_round_rect(x, y + 10.0, 3.0, h - 20.0, 1.5, pal.accent);
            } else if hovered {
                c.fill_round_rect(x, y, w, h, 5.0, pal.row_hover);
            }
            let icon = l.g.glyph(*glyph, 16.0)?;
            c.text(&icon, x + 14.0, y + (h - icon.height) / 2.0, pal.text);
            let label = l.g.text(&tr(name), 13.0, NORMAL, None)?;
            c.text(&label, x + 42.0, y + (h - label.height) / 2.0, pal.text);
        }
        l.push(x, y, w, h, Action::Page(i));
        y += h + 4.0;
    }

    // Page.
    let x = NAV_W + PAD;
    let title = l.g.text(&tr(PAGES[page].0), 22.0, SEMIBOLD, None)?;
    if let Some(c) = c {
        c.text(&title, x, PAD - 6.0, pal.text);
    }
    let mut y = PAD + title.height + 8.0;
    let p = &m.prefs;
    match page {
        0 => {
            y += l.toggle_card(
                y,
                "Start with Windows",
                "Start the app when you sign in",
                m.autostart,
                Action::Autostart,
            )?;
            let options: Vec<_> = settings::INTERVALS
                .iter()
                .map(|(secs, _)| (format!("{}m", secs / 60), Action::Interval(*secs), *secs == m.interval))
                .collect();
            y += l.choice_card(y, "Refresh usage every", "The usage API is rate-limited", &options)?;
            y += l.toggle_card(
                y,
                "Animations",
                "Glow, shake and the refresh spinner",
                p.animations,
                Action::Animations,
            )?;
            y += l.toggle_card(
                y,
                "GPG key timer",
                "How long git can still sign commits without asking",
                p.gpg_timer,
                Action::GpgTimer,
            )?;
            y += l.toggle_card(
                y,
                "Simulate a 5-hour limit",
                "On plans without one: a day's share of the week",
                p.simulate_five_hour,
                Action::SimulateFiveHour,
            )?;
            use crate::i18n::Choice;
            // The languages in their own names.
            let options = [
                ("System".to_string(), Action::Language(Choice::System), p.language == Choice::System),
                ("English".to_string(), Action::Language(Choice::English), p.language == Choice::English),
                ("Deutsch".to_string(), Action::Language(Choice::German), p.language == Choice::German),
            ];
            l.choice_card(y, "Language", "System: Windows' display language", &options)?;
        }
        1 => {
            // Lower cards than elsewhere: this page has the most of them, and
            // the displays below need the room.
            const H: f32 = COMPACT_CARD_H;
            let options = [
                ("Normal".to_string(), Action::Slim(false), !p.slim),
                ("Slim".to_string(), Action::Slim(true), p.slim),
            ];
            y += l.choice_card_h(y, H, "Display mode", "Slim: sessions on top, limits below in turn", &options)?;
            let turn_label = |s: u8| if s == 0 { "Off".to_string() } else { format!("{s} s") };
            let options: Vec<_> =
                settings::TURN_SECS.iter().map(|&s| (turn_label(s), Action::TurnSecs(s), s == p.turn_secs)).collect();
            y += l.choice_card_h(y, H, "Take turns", "Today's limit with the 5-hour one", &options)?;
            let options = [
                ("Automatic".to_string(), Action::Bars(Bars::Auto), m.bars == Bars::Auto),
                ("Always".to_string(), Action::Bars(Bars::Always), m.bars == Bars::Always),
                ("Never".to_string(), Action::Bars(Bars::Never), m.bars == Bars::Never),
            ];
            y += l.choice_card_h(y, H, "Usage bars", "Automatic: only when there's room", &options)?;
            y += l.toggle_card_h(
                y,
                H,
                "Solid background",
                "Cover taskbar buttons under the widget",
                m.solid,
                Action::Solid,
            )?;
            let options = [
                ("Main".to_string(), Action::Taskbars(Taskbars::Main), m.taskbars == Taskbars::Main),
                ("All".to_string(), Action::Taskbars(Taskbars::All), m.taskbars == Taskbars::All),
                ("Choose".to_string(), Action::Taskbars(Taskbars::Chosen), m.taskbars == Taskbars::Chosen),
            ];
            let desc = if m.more_monitors { "Windows must show taskbars there" } else { "With several displays" };
            y += l.choice_card_h(y, H, "Show on taskbars", desc, &options)?;
            if m.taskbars == Taskbars::Chosen {
                // With many displays the cards get lower (and lose their second
                // line), so all of them fit: the window doesn't scroll.
                // More than fit in one column: two or three side by side.
                let room = HEIGHT - 10.0 - y;
                let fits = |columns: usize| m.displays.len().div_ceil(columns) as f32 * (22.0 + 2.0) <= room;
                let columns = (1..=3).find(|c| fits(*c)).unwrap_or(3);
                let n = m.displays.len().div_ceil(columns) as f32;
                let (h, gap) = if n * (DISPLAY_CARD_H + GAP) > room {
                    ((room / n - 2.0).clamp(22.0, DISPLAY_CARD_H), 2.0)
                } else {
                    (DISPLAY_CARD_H, GAP)
                };
                let full = WIDTH - NAV_W - 2.0 * PAD;
                let col_w = (full - GAP * (columns as f32 - 1.0)) / columns as f32;
                let top = y;
                for (i, (id, name, detail)) in m.displays.iter().enumerate() {
                    let (col, row) = (i % columns, i / columns);
                    l.column = Some((NAV_W + PAD + col as f32 * (col_w + GAP), col_w));
                    let on = m.chosen.contains(id);
                    let detail = if h < 44.0 || columns > 1 { "" } else { detail.as_str() };
                    l.toggle_card_h(top + row as f32 * (h + gap), h, name, detail, on, Action::Display(id.clone()))?;
                }
                l.column = None;
                y = top + n * (h + gap);
            }
            let _ = y;
        }
        2 => {
            use crate::sysmon::{CPU, DISK, GPU, NET, RAM};
            let mon = p.monitor;
            // Off, or one of two ways to show the value.
            let ways = |bit: u8, on: bool, other: bool, a: &str, b: &str| {
                [
                    ("Off".to_string(), Action::MonitorShow(bit, None), !on),
                    (a.to_string(), Action::MonitorShow(bit, Some(false)), on && !other),
                    (b.to_string(), Action::MonitorShow(bit, Some(true)), on && other),
                ]
            };
            let h = DISPLAY_CARD_H;
            y += l.toggle_card_h(y, h, "System monitor", "Your PC's load in the taskbar", mon.on, Action::MonitorOn)?;
            if !mon.on {
                return Ok(l.hits);
            }
            y += l.toggle_card_h(y, h, "CPU", "Processor use", mon.cpu, Action::Monitor(CPU))?;
            let options = ways(RAM, mon.ram, mon.ram_gb, "%", "GB");
            y += l.choice_card_h(y, h, "Memory (RAM)", "In use, in percent or GB", &options)?;
            let options = ways(GPU, mon.gpu, mon.gpu_memory, "%", "GB");
            y += l.choice_card_h(y, h, "GPU", "Busiest engine (%) or video memory (GB)", &options)?;
            y += l.toggle_card_h(y, h, "Network", "Sent ↑ and received ↓ per second", mon.net, Action::Monitor(NET))?;
            let options = ways(DISK, mon.disk, mon.disk_rates, "%", "Read/write");
            y += l.choice_card_h(y, h, "Disk", "Busy time (%) or read and write rates", &options)?;
            l.toggle_card_h(
                y,
                DISPLAY_CARD_H,
                "Graphs",
                "The last 30 seconds behind each value",
                mon.graphs,
                Action::MonitorGraphs,
            )?;
        }
        3 => {
            let hooks_desc = match m.tracking {
                tracking::State::Outdated => "Needs repair: switch on to fix",
                _ => "Hooks: instant changes, agent counts",
            };
            y += l.toggle_card(y, "Instant updates", hooks_desc, m.tracking == tracking::State::On, Action::Hooks)?;
            y += l.toggle_card(
                y,
                "Session details",
                "Titles, tokens and cache timers",
                p.transcripts,
                Action::Transcripts,
            )?;
            y += l.toggle_card(y, "Sessions in WSL", "Running WSL distributions only", p.wsl, Action::Wsl)?;
            y += l.toggle_card(
                y,
                "Keep statistics",
                "Even after Claude Code deletes old transcripts",
                p.keep_stats,
                Action::KeepStats,
            )?;
            y += l.toggle_card(
                y,
                "Hide archived sessions",
                "Archived in the extension; shown again when they need you",
                p.hide_archived,
                Action::HideArchived,
            )?;
            let options: Vec<_> = settings::SESSION_POLLS
                .iter()
                .map(|(secs, _)| (format!("{secs}s"), Action::Poll(*secs), *secs == p.session_poll_secs))
                .collect();
            l.choice_card(y, "Check sessions every", "Less often uses less power", &options)?;
        }
        4 => {
            y += l.toggle_card(y, "Bubble", "Says which session is done or needs you", p.bubbles, Action::Bubbles)?;
            y += l.sound_card(y, "Sound when a session needs you", Kind::NeedsYou, &p.needs_sound)?;
            y += l.sound_card(y, "Sound when a session is done", Kind::Done, &p.done_sound)?;
            let options: Vec<_> = settings::VOLUMES
                .iter()
                .map(|(v, label)| (label.to_string(), Action::Volume(*v), *v == p.volume))
                .collect();
            let desc = if p.muted { "Muted: unmute from the right-click menu" } else { "For the built-in sounds" };
            l.choice_card(y, "Volume", desc, &options)?;
        }
        8 => {
            // The switch says how the connection is; the card below, where to.
            let status =
                m.mqtt_line.clone().unwrap_or_else(|| t("Limits, sessions and events to your broker").to_string());
            y += l.toggle_card(y, "MQTT", &status, p.mqtt, Action::Mqtt)?;
            let desc = p.mqtt_broker.clone().unwrap_or_else(|| t("None yet: copy one, then Paste").to_string());
            let mut options = vec![("Paste".to_string(), Action::PasteBroker, false)];
            if p.mqtt_broker.is_some() && p.mqtt {
                options.push(("Test".to_string(), Action::TestMqtt, false));
            }
            y += l.choice_card(y, "Broker", &desc, &options)?;
            let note = l.g.wrapped(
                t("Copy mqtt://user:password@host (or mqtts:// for TLS), then Paste. The password is kept encrypted for your Windows account. Home Assistant finds the entities by itself (MQTT discovery). Only names and states go out, no prompts."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            if let Some(c) = c {
                c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
            }
        }
        9 => {
            y += l.toggle_card(
                y,
                "Mailbox",
                "Mail between Claude sessions in a shared folder",
                p.mailbox,
                Action::Mailbox,
            )?;
            if p.mailbox {
                // How the folder stands: looked for (a turning arrow), in use (a
                // green tick), or missing (yellow, with how to set it).
                let (desc, style) = match (&m.mailbox_note, &m.mailbox_folder) {
                    (Some(note), _) => (note.clone(), DescStyle { color: pal.warn, spin: None }),
                    (None, MailboxFolder::Searching(angle)) => {
                        (t("Looking for it…").to_string(), DescStyle { color: pal.accent, spin: Some(*angle) })
                    }
                    (None, MailboxFolder::Found { path, set }) => {
                        let text = if *set { f("✓ {0}", &[path]) } else { f("✓ Found: {0}", &[path]) };
                        (text, DescStyle { color: pal.done, spin: None })
                    }
                    (None, MailboxFolder::Missing(Some(dir))) => {
                        (f("Not there: {0}", &[dir]), DescStyle { color: pal.warn, spin: None })
                    }
                    (None, MailboxFolder::Missing(None)) => (
                        t("Not found: copy its folder's path, then Paste").to_string(),
                        DescStyle { color: pal.warn, spin: None },
                    ),
                };
                let mut options = vec![("Paste".to_string(), Action::PasteMailbox, false)];
                if p.mailbox_dir.is_some() {
                    options.push(("Find it".to_string(), Action::FindMailbox, false));
                }
                l.desc_style = Some(style);
                y += l.choice_card(y, "Folder", &desc, &options)?;
                if !m.mailbox_inboxes.is_empty() {
                    y = inbox_list(&mut l, &m.mailbox_inboxes, x, y)?;
                }
            }
            let note = l.g.wrapped(
                t("For sessions that can't message each other, like one in Windows and one in WSL: they leave Markdown messages in each other's inbox folders. The flyout shows what's unread and what was handled; Trayward only reads the folder."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            // Below the inboxes when there's room.
            if let (Some(c), true) = (c, y + 8.0 + note.height <= HEIGHT - 8.0) {
                c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
            }
        }
        10 => {
            let version = env!("CARGO_PKG_VERSION");
            let (desc, mut options) = match &m.update {
                Some(u) => (
                    crate::i18n::f("Version {0} is out", &[&u.version]),
                    vec![("Download".to_string(), Action::Open(u.url.clone()), true)],
                ),
                None => (m.update_status.clone(), Vec::new()),
            };
            options.push(("Check now".to_string(), Action::CheckNow, false));
            y += l.choice_card(y, &format!("Trayward {version}"), &desc, &options)?;
            y += l.toggle_card(
                y,
                "Check for updates",
                "Once a day, asks GitHub for the newest release",
                p.update_check,
                Action::UpdateCheck,
            )?;
            let repo = crate::update::REPO_URL;
            let options = [("Open".to_string(), Action::Open(AUTHOR_URL.into()), false)];
            y += l.choice_card(y, &crate::i18n::f("Made by {0}", &[&AUTHOR]), "github.com/TheDelta", &options)?;
            let options = [
                ("Open".to_string(), Action::Open(repo.into()), false),
                ("Report a problem".to_string(), Action::Open(format!("{repo}/issues/new/choose")), false),
            ];
            y += l.choice_card(y, "Source code", "github.com/TheDelta/trayward · MIT license", &options)?;
            let note = l.g.wrapped(
                t("Trayward isn't made by or affiliated with Anthropic. Claude and Claude Code are trademarks of Anthropic."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            if let Some(c) = c {
                c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
            }
        }
        7 => {
            y += l.toggle_card(
                y,
                "Away mode",
                "While you're away, notifications go to your phone too",
                p.away,
                Action::Away,
            )?;
            let options: Vec<_> = settings::AWAY_MINUTES
                .iter()
                .map(|&m| (format!("{m}m"), Action::AwayAfter(m), m == p.away_minutes))
                .collect();
            y += l.choice_card(y, "Away after", "Minutes without mouse or keyboard", &options)?;
            y += l.toggle_card(y, "Done too", "Not only when a session needs you", p.away_done, Action::AwayDone)?;
            let target = p.push_url.as_deref().and_then(crate::push::parse);
            let desc = match (&m.push_status, &target) {
                (Some(s), _) => s.clone(),
                (None, Some(t)) => crate::push::describe(t),
                (None, None) => t("None yet: copy one, then Paste").to_string(),
            };
            let mut options = vec![("Paste".to_string(), Action::PasteUrl, false)];
            if target.is_some() {
                options.push(("Test".to_string(), Action::TestPush, false));
            }
            y += l.choice_card(y, "Push address", &desc, &options)?;
            let note = l.g.wrapped(
                t("An ntfy topic, a Discord or Slack webhook, or any https:// address that takes JSON. Only the session's name and state go out."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            if let Some(c) = c {
                c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
            }
        }
        6 => {
            // Always on: a card without a switch.
            l.card(y, CARD_H, "Claude Code", "Always on: limits, sessions and statistics", false, 0.0)?;
            y += CARD_H + GAP;
            let desc = if m.codex_found {
                "OpenAI's Codex CLI: its limits and sessions"
            } else {
                "OpenAI's Codex CLI: not used on this PC yet (no ~/.codex)"
            };
            y += l.toggle_card(y, "Codex", desc, p.codex, Action::Codex)?;
            if p.codex {
                use settings::Shown;
                let options = [
                    ("All".to_string(), Action::Shows(Shown::Both), p.taskbar_shows == Shown::Both),
                    ("Claude".to_string(), Action::Shows(Shown::Claude), p.taskbar_shows == Shown::Claude),
                    ("Codex".to_string(), Action::Shows(Shown::Codex), p.taskbar_shows == Shown::Codex),
                ];
                y += l.choice_card(y, "In the taskbar", "Whose limits the widget shows", &options)?;
            }
            let (sessions, limit) = (p.copilot, p.copilot && p.copilot_limit);
            let options = [
                ("Off".to_string(), Action::Copilot(false, false), !sessions),
                ("Sessions".to_string(), Action::Copilot(true, false), sessions && !limit),
                ("With limit".to_string(), Action::Copilot(true, true), limit),
            ];
            let desc = match &m.copilot_error {
                Some(why) if limit => why.as_str(),
                _ if m.copilot_found => "Its agent's sessions; the limit through gh",
                _ => "Not used on this PC yet (no ~/.copilot)",
            };
            y += l.choice_card(y, "Copilot", desc, &options)?;
            y += l.toggle_card(
                y,
                "Merge and pull requests",
                "GitLab and GitHub, through glab and gh",
                p.forge,
                Action::Forge,
            )?;
            if p.forge {
                y += l.toggle_card(
                    y,
                    "Pipeline bubbles",
                    "When a pipeline ends or a request is merged",
                    p.forge_bubbles,
                    Action::ForgeBubbles,
                )?;
            }
            let note = l.g.wrapped(
                t("Codex and Copilot are read from their own files, without signing in. Codex's limits are as of its last use. Copilot's limit and merge requests are asked through gh and glab."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            // With every card shown there's no room for it: the cards say enough.
            if y + 8.0 + note.height <= HEIGHT - 8.0 {
                if let Some(c) = c {
                    c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
                }
            }
        }
        _ => {
            let label = |v: u8| if v == 0 { "Off".to_string() } else { format!("{v} %") };
            let options: Vec<_> =
                settings::WARN_WEEKLY.iter().map(|&v| (label(v), Action::WarnWeekly(v), v == p.warn_weekly)).collect();
            // Lower cards than elsewhere: this page has the most of them.
            const H: f32 = COMPACT_CARD_H;
            y += l.choice_card_h(y, H, "Weekly limit", "A bubble at this level, and again at 95 %", &options)?;
            let options: Vec<_> = settings::WARN_FIVE_HOUR
                .iter()
                .map(|&v| (label(v), Action::WarnFiveHour(v), v == p.warn_five_hour))
                .collect();
            y += l.choice_card_h(y, H, "5-hour limit", "A bubble at this level", &options)?;
            use crate::forecast::Budget;
            let options = [
                ("Adaptive".to_string(), Action::DailyBudget(Budget::Adaptive), p.daily_budget == Budget::Adaptive),
                ("Even".to_string(), Action::DailyBudget(Budget::Even), p.daily_budget == Budget::Even),
            ];
            y += l.choice_card_h(y, H, "Daily budget", "Adaptive: what a day leaves goes to the next", &options)?;
            y += l.toggle_card_h(
                y,
                H,
                "Pace",
                "A bubble when your pace would use up the week before it resets",
                p.warn_pace,
                Action::WarnPace,
            )?;
            let options: Vec<_> = settings::COUNTDOWN_AT
                .iter()
                .map(|&v| {
                    (if v == 100 { "Full".to_string() } else { label(v) }, Action::CountdownAt(v), v == p.countdown_at)
                })
                .collect();
            y += l.choice_card_h(y, H, "Countdown", "Time until the reset instead of %, from", &options)?;
            y += l.toggle_card_h(
                y,
                H,
                "Free again",
                "A bubble when a limit that was at 80 % or more resets",
                p.notify_free,
                Action::NotifyFree,
            )?;
            l.toggle_card_h(
                y,
                H,
                "Weekly summary",
                "A bubble on Monday morning about last week",
                p.week_summary,
                Action::WeekSummary,
            )?;
        }
    }
    Ok(l.hits)
}

/// Most inboxes the Mailbox page lists.
const INBOX_ROWS: usize = 7;

/// The mailbox's inboxes as a table: inbox, unread (and how long the oldest
/// waited), handled, last handled, whether a session runs for it. Returns where it ends.
fn inbox_list(l: &mut Layout, rows: &[InboxRow], x: f32, y: f32) -> Result<f32> {
    let pal = l.pal;
    let w = WIDTH - x - PAD;
    // Column starts, from the card's left edge.
    let cols = [12.0, w * 0.36, w * 0.56, w * 0.72, w * 0.88];
    let heads = ["Inbox", "Unread", "Handled", "Last handled", "Session"];
    let mut y = y + 2.0;
    if let Some(c) = l.c {
        for (head, cx) in heads.iter().zip(cols) {
            let t = l.g.text(&tr(head), 11.0, NORMAL, None)?;
            c.text(&t, x + cx, y, pal.text_dim);
        }
    }
    y += 18.0;
    let shown = rows.len().min(INBOX_ROWS);
    if let Some(c) = l.c {
        c.fill_round_rect(x, y - 2.0, w, shown as f32 * 22.0 + 4.0, 6.0, pal.card);
    }
    for row in rows.iter().take(INBOX_ROWS) {
        let name = l.g.text(&row.name, 12.5, SEMIBOLD, Some(cols[1] - cols[0] - 8.0))?;
        let unread = match (&row.waiting, row.unread) {
            (_, 0) => "0".to_string(),
            (Some(waited), n) => f("{0} · {1}", &[&n, waited]),
            (None, n) => n.to_string(),
        };
        let unread = l.g.text(&unread, 12.0, if row.unread > 0 { SEMIBOLD } else { NORMAL }, None)?;
        let done = l.g.text(&row.done.to_string(), 12.0, NORMAL, None)?;
        let last = l.g.text(row.last_done.as_deref().map_or("–", |s| s), 12.0, NORMAL, None)?;
        let session = l.g.text(if row.session { t("running") } else { "–" }, 12.0, NORMAL, None)?;
        if let Some(c) = l.c {
            let mid = y + 9.0;
            let at = |t: &gfx::Text| mid - t.height / 2.0;
            c.text(&name, x + cols[0], at(&name), pal.text);
            c.text(&unread, x + cols[1], at(&unread), if row.unread > 0 { pal.warn } else { pal.text_dim });
            c.text(&done, x + cols[2], at(&done), pal.text_dim);
            c.text(&last, x + cols[3], at(&last), pal.text_dim);
            if row.session {
                c.fill_circle(x + cols[4] + 4.0, mid, 3.5, pal.done);
                c.text(&session, x + cols[4] + 12.0, at(&session), pal.text);
            } else {
                c.text(&session, x + cols[4], at(&session), pal.text_dim);
            }
        }
        y += 22.0;
    }
    if rows.len() > INBOX_ROWS {
        let more = l.g.text(&f("{0} more", &[&(rows.len() - INBOX_ROWS)]), 11.0, NORMAL, None)?;
        if let Some(c) = l.c {
            c.text(&more, x + cols[0], y + 4.0, pal.text_dim);
        }
        y += more.height + 4.0;
    }
    Ok(y + 4.0)
}

pub struct SettingsWindow {
    pub hwnd: Option<HWND>,
    pub page: usize,
    model: Option<Model>,
    hits: Vec<Hit>,
    hover: Option<usize>,
}

fn scale_of(hwnd: HWND) -> f32 {
    // SAFETY: plain query on our own window.
    unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0
}

impl SettingsWindow {
    pub const fn new() -> SettingsWindow {
        SettingsWindow { hwnd: None, page: 0, model: None, hits: Vec::new(), hover: None }
    }

    pub fn visible(&self) -> bool {
        // SAFETY: plain query on our own window.
        self.hwnd.is_some_and(|h| unsafe { IsWindowVisible(h) }.as_bool())
    }

    /// Window size for a client area of WIDTH × HEIGHT DIPs at `dpi`.
    fn window_size(dpi: u32) -> (i32, i32) {
        let s = dpi as f32 / 96.0;
        let mut r = RECT { left: 0, top: 0, right: (WIDTH * s).ceil() as i32, bottom: (HEIGHT * s).ceil() as i32 };
        // SAFETY: valid rectangle.
        let _ = unsafe { AdjustWindowRectExForDpi(&mut r, STYLE, false, WINDOW_EX_STYLE(0), dpi) };
        (r.right - r.left, r.bottom - r.top)
    }

    /// Opens the window (or brings it to the front), centred on the monitor of `anchor`.
    /// The language changed: the title follows it.
    pub fn retitle(&self) {
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window; the title is copied.
            let _ = unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                    hwnd,
                    &windows::core::HSTRING::from(t("Trayward Settings")),
                )
            };
        }
    }

    pub fn open(&mut self, model: Model, anchor: RECT, dpi: u32) {
        if self.hwnd.is_none() {
            // SAFETY: plain window creation with a registered class.
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    CLASS,
                    &windows::core::HSTRING::from(t("Trayward Settings")),
                    STYLE,
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
            self.hwnd = hwnd.ok();
        }
        let Some(hwnd) = self.hwnd else { return };
        // SAFETY: our own window; the title is copied. Set again: the language may have changed.
        let _ = unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                hwnd,
                &windows::core::HSTRING::from(t("Trayward Settings")),
            )
        };
        let was_visible = self.visible();
        self.set_model(model);
        // SAFETY: Win32 calls on our own window with valid out-pointers.
        unsafe {
            if !was_visible {
                let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST), &mut mi);
                let work = mi.rcWork;
                let (w, h) = Self::window_size(dpi);
                let x = work.left + ((work.right - work.left) - w) / 2;
                let y = work.top + ((work.bottom - work.top) - h) / 2;
                let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    pub fn close(&mut self) {
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
        self.hover = None;
    }

    /// Shows new values, repainting only when something changed.
    pub fn set_model(&mut self, model: Model) {
        let Some(hwnd) = self.hwnd else { return };
        if self.model.as_ref() == Some(&model) {
            return;
        }
        // SAFETY: attribute value of the documented size; our own window.
        unsafe {
            let dark = BOOL::from(model.dark);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const _,
                std::mem::size_of::<BOOL>() as u32,
            );
        }
        self.hits = render(None, &model, self.page, None).unwrap_or_default();
        self.model = Some(model);
        self.invalidate();
    }

    pub fn set_page(&mut self, page: usize) {
        if page < PAGES.len() && page != self.page {
            self.page = page;
            self.hover = None;
            if let Some(m) = &self.model {
                self.hits = render(None, m, page, None).unwrap_or_default();
            }
            self.invalidate();
        }
    }

    fn invalidate(&self) {
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window.
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        }
    }

    fn find(&self, x: i32, y: i32) -> Option<usize> {
        let s = scale_of(self.hwnd?);
        let (x, y) = (x as f32 / s, y as f32 / s);
        crate::hits::find(&self.hits, x, y)
    }

    pub fn set_mouse(&mut self, pos: Option<(i32, i32)>) {
        let hover = pos.and_then(|(x, y)| self.find(x, y));
        if hover != self.hover {
            self.hover = hover;
            self.invalidate();
        }
    }

    pub fn hovering(&self) -> bool {
        self.hover.is_some()
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        self.find(x, y).map(|i| self.hits[i].action.clone())
    }

    pub fn model(&self) -> Option<&Model> {
        self.model.as_ref()
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
                let bg: Color = pal.window;
                let _ = gfx::paint_window(hwnd, rc.right, rc.bottom, 96.0 * scale_of(hwnd), bg, |c| {
                    let _ = render(Some(c), m, self.page, self.hover);
                });
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }
}

/// Asks for a WAV file with the standard Open dialog. Modal: run it outside
/// event handling.
pub fn pick_wav(owner: HWND) -> Option<String> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let mut buf = [0u16; 1024];
    // The filter is "name\0pattern\0\0"; both in the app's language.
    let filter: Vec<u16> = format!("{}\0*.wav\0\0", t("Sounds (*.wav)")).encode_utf16().collect();
    let title: Vec<u16> = t("Choose a sound").encode_utf16().chain(Some(0)).collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: the struct points at `buf`, which outlives the modal call.
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// German translations of the settings window's text (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("Trayward Settings", "Trayward-Einstellungen"),
    ("General", "Allgemein"),
    ("Taskbar", "Taskleiste"),
    ("Notifications", "Benachrichtigungen"),
    ("Limits", "Limits"),
    ("System monitor", "Systemmonitor"),
    ("Your PC's load in the taskbar", "Die Auslastung des PCs in der Taskleiste"),
    ("CPU", "CPU"),
    ("Processor use", "Prozessorauslastung"),
    ("Memory (RAM)", "Arbeitsspeicher (RAM)"),
    ("In use, in percent or GB", "Belegt, in Prozent oder GB"),
    ("GPU", "GPU"),
    ("Busiest engine (%) or video memory (GB)", "Meistgenutzte Engine (%) oder Grafikspeicher (GB)"),
    ("Network", "Netzwerk"),
    ("Sent ↑ and received ↓ per second", "Gesendet ↑ und empfangen ↓ pro Sekunde"),
    ("Disk", "Datenträger"),
    ("Busy time (%) or read and write rates", "Aktive Zeit (%) oder Lese-/Schreibrate"),
    ("Read/write", "Lesen/Schreiben"),
    ("Graphs", "Verlauf"),
    ("The last 30 seconds behind each value", "Die letzten 30 Sekunden hinter jedem Wert"),
    ("Providers", "Anbieter"),
    ("No sound", "Kein Ton"),
    ("Custom", "Eigener"),
    ("Start with Windows", "Mit Windows starten"),
    ("Start the app when you sign in", "Die App bei der Anmeldung starten"),
    ("Refresh usage every", "Nutzung aktualisieren alle"),
    ("The usage API is rate-limited", "Die Nutzungs-API ist begrenzt"),
    ("Animations", "Animationen"),
    ("Slim", "Schmal"),
    ("Slim: sessions on top, limits below in turn", "Schmal: oben Sitzungen, unten Limits im Wechsel"),
    ("Take turns", "Im Wechsel"),
    ("Mailbox", "Postfach"),
    ("Mail between Claude sessions in a shared folder", "Post zwischen Claude-Sitzungen in einem gemeinsamen Ordner"),
    ("Not there: {0}", "Nicht vorhanden: {0}"),
    ("Not found: copy its folder's path, then Paste", "Nicht gefunden: Pfad des Ordners kopieren, dann Einfügen"),
    ("Find it", "Suchen"),
    ("Folder", "Ordner"),
    ("Looking for it…", "Wird gesucht…"),
    ("✓ {0}", "✓ {0}"),
    ("✓ Found: {0}", "✓ Gefunden: {0}"),
    ("Inbox", "Postfach"),
    ("Unread", "Ungelesen"),
    ("Handled", "Erledigt"),
    ("Last handled", "Zuletzt erledigt"),
    ("Session", "Sitzung"),
    ("running", "läuft"),
    ("{0} · {1}", "{0} · {1}"),
    ("{0} more", "{0} weitere"),
    ("For sessions that can't message each other, like one in Windows and one in WSL: they leave Markdown messages in each other's inbox folders. The flyout shows what's unread and what was handled; Trayward only reads the folder.", "Für Sitzungen, die sich keine Nachrichten schicken können, etwa eine in Windows und eine in WSL: Sie legen einander Markdown-Nachrichten in die Postfach-Ordner. Das Flyout zeigt, was ungelesen ist und was erledigt wurde; Trayward liest den Ordner nur."),
    ("Daily budget", "Tagesbudget"),
    ("Adaptive: what a day leaves goes to the next", "Anpassend: Rest geht an die nächsten Tage"),
    ("Adaptive", "Anpassend"),
    ("Even", "Gleich"),
    ("Today's limit with the 5-hour one", "Tageslimit im Wechsel mit 5 Stunden"),
    ("Glow, shake and the refresh spinner", "Leuchten, Wackeln und der Aktualisieren-Kreisel"),
    ("GPG key timer", "GPG-Schlüssel-Timer"),
    ("How long git can still sign commits without asking", "Wie lange git noch ohne Nachfrage signieren kann"),
    ("Simulate a 5-hour limit", "5-Stunden-Limit simulieren"),
    ("On plans without one: a day's share of the week", "Bei Plänen ohne: ein Tagesanteil der Woche"),
    ("Language", "Sprache"),
    ("System: Windows' display language", "System: die Anzeigesprache von Windows"),
    ("Automatic", "Automatisch"),
    ("Always", "Immer"),
    ("Never", "Nie"),
    ("Usage bars", "Nutzungsbalken"),
    ("Automatic: only when there's room", "Automatisch: nur wenn Platz ist"),
    ("Solid background", "Fester Hintergrund"),
    ("Cover taskbar buttons under the widget", "Taskleisten-Schaltflächen unter dem Widget verdecken"),
    ("Main", "Haupt"),
    ("All", "Alle"),
    ("Choose", "Auswählen"),
    ("Windows must show taskbars there", "Windows muss dort Taskleisten zeigen"),
    ("With several displays", "Bei mehreren Bildschirmen"),
    ("Show on taskbars", "Auf Taskleisten zeigen"),
    ("Needs repair: switch on to fix", "Reparatur nötig: zum Beheben einschalten"),
    ("Hooks: instant changes, agent counts", "Hooks: sofortige Änderungen, Agentenzahl"),
    ("Instant updates", "Sofort-Updates"),
    ("Session details", "Sitzungsdetails"),
    ("Titles, tokens and cache timers", "Titel, Token und Cache-Timer"),
    ("Sessions in WSL", "Sitzungen in WSL"),
    ("Running WSL distributions only", "Nur laufende WSL-Distributionen"),
    ("Hide archived sessions", "Archivierte Sitzungen ausblenden"),
    ("Archived in the extension; shown again when they need you", "In der Erweiterung archiviert; sichtbar, wenn sie dich brauchen"),
    ("Check sessions every", "Sitzungen prüfen alle"),
    ("Less often uses less power", "Seltener spart Strom"),
    ("Bubble", "Sprechblase"),
    ("Says which session is done or needs you", "Sagt, welche Sitzung fertig ist oder dich braucht"),
    ("Sound when a session needs you", "Ton, wenn eine Sitzung dich braucht"),
    ("Sound when a session is done", "Ton, wenn eine Sitzung fertig ist"),
    ("Muted: unmute from the right-click menu", "Stumm: im Rechtsklick-Menü wieder einschalten"),
    ("For the built-in sounds", "Für die eingebauten Töne"),
    ("Volume", "Lautstärke"),
    ("Quiet", "Leise"),
    ("Medium", "Mittel"),
    ("Loud", "Laut"),
    ("Always on: limits, sessions and statistics", "Immer an: Limits, Sitzungen und Statistik"),
    ("OpenAI's Codex CLI: its limits and sessions", "Codex CLI von OpenAI: seine Limits und Sitzungen"),
    ("OpenAI's Codex CLI: not used on this PC yet (no ~/.codex)", "Codex CLI von OpenAI: hier noch nicht genutzt"),
    ("Both", "Beide"),
    ("In the taskbar", "In der Taskleiste"),
    ("Whose limits the widget shows", "Wessen Limits das Widget zeigt"),
    ("Codex and Copilot are read from their own files, without signing in. Codex's limits are as of its last use. Copilot's limit and merge requests are asked through gh and glab.", "Codex und Copilot werden aus ihren eigenen Dateien gelesen, ohne Anmeldung. Die Limits von Codex sind vom letzten Nutzen. Das Limit von Copilot und Merge-Requests werden über gh und glab abgefragt."),
    ("Sounds (*.wav)", "Töne (*.wav)"),
    ("Choose a sound", "Ton auswählen"),
    ("Its agent's sessions; the limit through gh", "Sitzungen des Agenten; das Limit über gh"),
    ("With limit", "Mit Limit"),
    ("Not used on this PC yet (no ~/.copilot)", "Hier noch nicht genutzt (kein ~/.copilot)"),
    ("github.com/TheDelta/trayward · MIT license", "github.com/TheDelta/trayward · MIT-Lizenz"),
    ("All", "Alle"),
    ("Weekly limit", "Wochenlimit"),
    ("A bubble at this level, and again at 95 %", "Sprechblase bei diesem Stand und bei 95 %"),
    ("5-hour limit", "5-Stunden-Limit"),
    ("A bubble at this level", "Sprechblase bei diesem Stand"),
    ("Pace", "Tempo"),
    ("A bubble when your pace would use up the week before it resets", "Wenn dein Tempo die Woche zu früh aufbraucht"),
    ("Full", "Voll"),
    ("Countdown", "Countdown"),
    ("Time until the reset instead of %, from", "Zeit bis zum Reset statt %, ab"),
    ("Free again", "Wieder frei"),
    ("A bubble when a limit that was at 80 % or more resets", "Sprechblase, wenn ein Limit ab 80 % zurückgesetzt wird"),
    ("Each warning comes once per limit window. While you present or have Do not disturb on, it waits.", "Jede Warnung kommt einmal pro Limit-Zeitraum. Während du präsentierst oder „Nicht stören“ an ist, wartet sie."),
    ("Knock", "Klopfen"),
    ("Ping", "Ping"),
    ("Nudge", "Stups"),
    ("Chime", "Glocke"),
    ("Marimba", "Marimba"),
    ("Arpeggio", "Arpeggio"),
    ("Two soft taps on wood", "Zwei sanfte Klopfer auf Holz"),
    ("A single gentle bell", "Eine einzelne sanfte Glocke"),
    ("A small step up, like \"hm?\"", "Ein kleiner Schritt nach oben, wie „hm?“"),
    ("A bright rising fifth", "Eine helle, steigende Quinte"),
    ("Warm marimba, landing home", "Warme Marimba, die heimkehrt"),
    ("A short major chord", "Ein kurzer Dur-Akkord"),
    ("Merge and pull requests", "Merge- und Pull-Requests"),
    ("GitLab and GitHub, through glab and gh", "GitLab und GitHub, über glab und gh"),
    ("Pipeline bubbles", "Pipeline-Sprechblasen"),
    ("When a pipeline ends or a request is merged", "Wenn eine Pipeline endet oder gemergt wird"),
    ("Weekly summary", "Wochenrückblick"),
    ("A bubble on Monday morning about last week", "Montagmorgens eine Sprechblase zur letzten Woche"),
    ("Away", "Abwesend"),
    ("Away mode", "Abwesenheitsmodus"),
    ("While you're away, notifications go to your phone too", "Bist du weg, gehen Hinweise auch ans Handy"),
    ("Away after", "Abwesend nach"),
    ("Minutes without mouse or keyboard", "Minuten ohne Maus und Tastatur"),
    ("Done too", "Auch „fertig“"),
    ("Not only when a session needs you", "Nicht nur, wenn eine Sitzung dich braucht"),
    ("None yet: copy one, then Paste", "Noch keine: kopieren, dann Einfügen"),
    ("Paste", "Einfügen"),
    ("Test", "Testen"),
    ("Push address", "Push-Adresse"),
    ("An ntfy topic, a Discord or Slack webhook, or any https:// address that takes JSON. Only the session's name and state go out.", "Ein ntfy-Topic, ein Discord- oder Slack-Webhook oder jede https://-Adresse, die JSON annimmt. Es gehen nur Name und Zustand der Sitzung raus."),
    ("About", "Über"),
    ("Version {0} is out", "Version {0} ist da"),
    ("Download", "Herunterladen"),
    ("Check now", "Jetzt prüfen"),
    ("Check for updates", "Nach Updates suchen"),
    ("Once a day, asks GitHub for the newest release", "Fragt einmal am Tag GitHub nach der neuesten Version"),
    ("Made by {0}", "Von {0}"),
    ("Open", "Öffnen"),
    ("Report a problem", "Problem melden"),
    ("Source code", "Quellcode"),
    ("Trayward isn't made by or affiliated with Anthropic. Claude and Claude Code are trademarks of Anthropic.", "Trayward ist nicht von Anthropic und nicht mit Anthropic verbunden. Claude und Claude Code sind Marken von Anthropic."),
    ("Keep statistics", "Statistik behalten"),
    ("Even after Claude Code deletes old transcripts", "Auch wenn Claude Code alte Transkripte löscht"),
    ("Limits, sessions and events to your broker", "Limits, Sitzungen und Ereignisse an deinen Broker"),
    ("Broker", "Broker"),
    ("Copy mqtt://user:password@host (or mqtts:// for TLS), then Paste. The password is kept encrypted for your Windows account. Home Assistant finds the entities by itself (MQTT discovery). Only names and states go out, no prompts.", "Kopiere mqtt://user:passwort@host (oder mqtts:// für TLS), dann Einfügen. Das Passwort wird für dein Windows-Konto verschlüsselt gespeichert. Home Assistant findet die Entitäten von selbst (MQTT Discovery). Es gehen nur Namen und Zustände raus, keine Prompts."),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, Choice};

    /// Settings with everything that adds a card: Codex on, chosen displays.
    fn model() -> Model {
        Model {
            dark: true,
            prefs: Prefs {
                forge: true,
                push_url: Some("https://ntfy.sh/my-phone".into()),
                mqtt: true,
                mqtt_broker: Some("mqtt://ha@homeassistant.local:1883".into()),
                // On: the page with all its cards.
                monitor: crate::settings::Monitor { on: true, ..Prefs::defaults().monitor },
                ..Prefs::defaults()
            },
            interval: 120,
            bars: Bars::Auto,
            solid: false,
            taskbars: Taskbars::Chosen,
            chosen: vec!["a".into()],
            displays: vec![
                ("a".into(), "DELL U2723QE".into(), "Main display · 3840 × 2160".into()),
                ("b".into(), "LG 27GP850".into(), "Right of the main display · 2560 × 1440".into()),
            ],
            more_monitors: true,
            codex_found: false,
            copilot_found: false,
            copilot_error: Some("gh: program not found".into()),
            tracking: tracking::State::Outdated,
            autostart: true,
            push_status: Some("Not sent: The server answered 404".into()),
            update: Some(crate::update::Release { version: "0.12.0".into(), url: "https://example.com".into() }),
            mqtt_line: Some("Not connected: wrong user name or password".into()),
            update_status: "Up to date".into(),
            mailbox_folder: MailboxFolder::Found { path: r"C:\work\.claude-mailbox".into(), set: false },
            mailbox_note: None,
            mailbox_inboxes: (0..9)
                .map(|i| InboxRow {
                    name: format!("project-number-{i}"),
                    unread: i,
                    waiting: (i > 0).then(|| "14h".into()),
                    done: 26,
                    last_done: Some("2h".into()),
                    session: i % 2 == 0,
                })
                .collect(),
        }
    }

    /// Texts of the settings that read the same in German.
    const SAME_IN_GERMAN: &[&str] = &[
        "System",
        "English",
        "Deutsch",
        "MQTT",
        "Claude",
        "Claude Code",
        "Codex",
        "Copilot",
        "GB",
        "%",
        "github.com/TheDelta",
    ];

    #[test]
    fn many_displays_fit() {
        i18n::set(Choice::English);
        let page = PAGES.iter().position(|(name, _)| *name == "Taskbar").unwrap();
        for n in [3, 4, 6, 8, 12] {
            let displays = (0..n).map(|i| (format!("d{i}"), format!("Display {i}"), "Somewhere · 2560 × 1440".into()));
            let m = Model { displays: displays.collect(), ..model() };
            let hits = render(None, &m, page, None).unwrap();
            let bottom = hits.iter().map(|h| h.y + h.h).fold(0.0, f32::max);
            assert!(bottom <= HEIGHT, "{n} displays end at {bottom}, the window at {HEIGHT}");
            // Each display can still be switched.
            let switches = hits.iter().filter(|h| matches!(h.action, Action::Display(_))).count();
            assert_eq!(switches, n);
            gfx::take_cut();
        }
    }

    #[test]
    fn every_page_fits_in_both_languages() {
        let code = include_str!("settings_window.rs").split("#[cfg(test)]\nmod tests").next().unwrap_or_default();
        let mut untranslated = Vec::new();
        for lang in [Choice::English, Choice::German] {
            i18n::set(lang);
            for (page, (name, _)) in PAGES.iter().enumerate() {
                i18n::take_untranslated();
                let hits = render(None, &model(), page, None).unwrap();
                let cut = gfx::take_cut();
                assert!(cut.is_empty(), "{lang:?}, {name}: cut off: {cut:?}");
                // Card titles, descriptions and choices aren't literals for the
                // scan of the code: what's shown in German must be German, but
                // for what is the same in both (names, numbers, units).
                // Only texts written in this file: values (names, versions, messages) come from elsewhere.
                let mut english: Vec<String> = i18n::take_untranslated();
                english.retain(|s| !SAME_IN_GERMAN.contains(&s.as_str()) && code.contains(&format!("{s:?}")));
                if lang == Choice::German {
                    untranslated.extend(english.into_iter().map(|s| format!("{name}: {s}")));
                }
                // Every control is inside the window.
                let bottom = hits.iter().map(|h| h.y + h.h).fold(0.0, f32::max);
                assert!(bottom <= HEIGHT, "{lang:?}, {name}: ends at {bottom}, the window at {HEIGHT}");
            }
        }
        untranslated.sort();
        untranslated.dedup();
        assert!(
            untranslated.is_empty(),
            "no German translation for:
{}",
            untranslated.join(
                "
"
            )
        );
    }
}
