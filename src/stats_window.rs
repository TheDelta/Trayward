//! The statistics window: an overview of any period, every session in detail,
//! lists of tool calls, failures, files and past sessions, and disk usage.
//! Same look as the settings. Charts show values on hover; pages scroll.

use crate::alerts::Week;
use crate::forecast;
use crate::gfx::{self, Canvas, NORMAL, SEMIBOLD};
use crate::i18n::{self, f, t};
use crate::sessions::Status;
use crate::setup::{Part, Setup};
use crate::stats::{
    self, Activity, CallRow, DAY, Failure, FileRow, HOUR, Overview, Period, SessionData, SessionInfo, Storage, Turn,
};
use crate::theme::{self, Color, Palette};
use crate::timefmt;
use crate::transcripts::tokens;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    PAINTSTRUCT,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, IsWindowVisible, KillTimer, SW_HIDE, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
    SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CAPTION,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
};
use windows::core::{BOOL, HSTRING, PCWSTR, Result, w};

pub const CLASS: PCWSTR = w!("Trayward.Stats");

const WIDTH: f32 = 1000.0;
const HEIGHT: f32 = 720.0;
/// How high the window's content area really is (DIPs): `HEIGHT`, or less on
/// a screen that hasn't the room. Scrolling and what's drawn go by this.
static VIEW_H: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(HEIGHT.to_bits());

/// Counts up whenever the page's data is replaced: what was worked out from
/// the old data (see [`memo`]) is forgotten then.
static DATA_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn new_data() {
    DATA_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

type Memo = (u64, std::collections::HashMap<u64, std::rc::Rc<dyn std::any::Any>>);
thread_local! {
    static MEMO: RefCell<Memo> = RefCell::new((0, std::collections::HashMap::new()));
}

/// What `make` works out from the page's data, kept until the data is
/// replaced: the page is laid out and drawn many times a second (hover,
/// scrolling), and tallies, filters and sorts over every tool call of a
/// period don't belong in each of those. `key` names what is worked out and
/// everything it depends on besides the data (filter, search, order).
fn memo<T: 'static>(key: &impl std::hash::Hash, make: impl FnOnce() -> T) -> std::rc::Rc<T> {
    // Tests draw different data without replacing a window's: nothing is kept.
    if cfg!(test) {
        return std::rc::Rc::new(make());
    }
    memo_at(DATA_GEN.load(std::sync::atomic::Ordering::Relaxed), key, make)
}

fn memo_at<T: 'static>(generation: u64, key: &impl std::hash::Hash, make: impl FnOnce() -> T) -> std::rc::Rc<T> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (std::any::TypeId::of::<T>(), key).hash(&mut h);
    let k = h.finish();
    let kept = MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if m.0 != generation {
            *m = (generation, std::collections::HashMap::new());
        }
        m.1.get(&k).cloned().and_then(|v| v.downcast::<T>().ok())
    });
    kept.unwrap_or_else(|| {
        // Not borrowed while it's made: `make` may keep things of its own.
        let made = std::rc::Rc::new(make());
        MEMO.with(|m| {
            let mut m = m.borrow_mut();
            // Each search typed is a key of its own: don't keep them all.
            if m.1.len() >= MEMO_MAX {
                m.1.clear();
            }
            m.1.insert(k, made.clone());
        });
        made
    })
}

/// Most lists [`memo`] keeps at once.
const MEMO_MAX: usize = 16;

fn view_h() -> f32 {
    f32::from_bits(VIEW_H.load(std::sync::atomic::Ordering::Relaxed))
}
const NAV_W: f32 = 250.0;
const PAD: f32 = 24.0;
const GAP: f32 = 12.0;
const X0: f32 = NAV_W + PAD;
const W: f32 = WIDTH - X0 - PAD;
/// Height of a chart card.
const CHART_H: f32 = 250.0;
/// One mouse-wheel notch.
const WHEEL_STEP: f32 = 72.0;
/// Active sessions listed in the navigation.
const NAV_SESSIONS: usize = 9;

const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);
const CHART: char = '\u{E9D2}';
const LIST: char = '\u{E8FD}';
const DRIVE: char = '\u{EDA2}';
const CALENDAR: char = '\u{E787}';
const FOLDER: char = '\u{E838}';
const BACK: char = '\u{E72B}';
const CHEVRON: char = '\u{E76C}';
const SEARCH: char = '\u{E721}';
const DOWNLOAD: char = '\u{E896}';
const COPY: char = '\u{E8C8}';
const CHECK: char = '\u{E73E}';
/// Monday 2026-11-09, noon UTC: for weekday names.
const A_MONDAY: i64 = 1_794_225_600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Failed,
    Tool(String),
    /// Failures with this [`stats::Call::signature`].
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Overview(Period),
    Session(String),
    /// Tool calls of a session, or of all sessions in a period.
    Calls {
        session: Option<String>,
        period: Period,
        filter: Filter,
    },
    /// Files read or changed by a session, or by all sessions in a period.
    Files {
        session: Option<String>,
        period: Period,
    },
    /// Every session; `archived` filters.
    Sessions {
        archived: Option<bool>,
    },
    /// Every project.
    Projects,
    /// One project's sessions in a period: the overview of them.
    Project {
        name: String,
        period: Period,
    },
    Storage,
    /// Every week, summed up.
    Weeks,
}

impl Page {
    /// The page whose data this one shows: filters don't need new data.
    pub fn data_key(&self) -> Page {
        match self {
            Page::Calls { session, period, .. } => {
                Page::Calls { session: session.clone(), period: *period, filter: Filter::All }
            }
            Page::Sessions { .. } => Page::Sessions { archived: None },
            p => p.clone(),
        }
    }

    /// Whether this page belongs to the navigation entry `target`.
    fn under(&self, target: &Page) -> bool {
        match (self, target) {
            (
                Page::Overview(_) | Page::Calls { session: None, .. } | Page::Files { session: None, .. },
                Page::Overview(_),
            ) => true,
            (Page::Sessions { .. }, Page::Sessions { .. }) | (Page::Storage, Page::Storage) => true,
            (Page::Weeks, Page::Weeks) => true,
            (Page::Projects | Page::Project { .. }, Page::Projects) => true,
            (
                Page::Session(a) | Page::Calls { session: Some(a), .. } | Page::Files { session: Some(a), .. },
                Page::Session(b),
            ) => a == b,
            _ => false,
        }
    }
}

/// What the list of all sessions is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSort {
    Recent,
    Prompts,
    Tokens,
    Cost,
    Size,
    Length,
    Failures,
    Title,
}

impl SessionSort {
    const ALL: [SessionSort; 8] = [
        SessionSort::Recent,
        SessionSort::Prompts,
        SessionSort::Tokens,
        SessionSort::Cost,
        SessionSort::Size,
        SessionSort::Length,
        SessionSort::Failures,
        SessionSort::Title,
    ];

    fn label(self) -> &'static str {
        t(match self {
            SessionSort::Recent => "Recent",
            SessionSort::Prompts => "Prompts",
            SessionSort::Tokens => "Tokens",
            SessionSort::Cost => "API value",
            SessionSort::Size => "Size",
            SessionSort::Length => "Duration",
            SessionSort::Failures => "Failures",
            SessionSort::Title => "Title",
        })
    }
}

/// What the list of files is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSort {
    Edits,
    Lines,
    Reads,
    Recent,
    Name,
}

impl FileSort {
    const ALL: [FileSort; 5] = [FileSort::Edits, FileSort::Lines, FileSort::Reads, FileSort::Recent, FileSort::Name];

    fn label(self) -> &'static str {
        t(match self {
            FileSort::Edits => "Edits",
            FileSort::Lines => "Lines",
            FileSort::Reads => "Reads",
            FileSort::Recent => "Recent",
            FileSort::Name => "Name",
        })
    }
}

/// What the list of projects is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectSort {
    Recent,
    Sessions,
    Prompts,
    Tokens,
    Cost,
    Failures,
    Name,
}

impl ProjectSort {
    const ALL: [ProjectSort; 7] = [
        ProjectSort::Recent,
        ProjectSort::Sessions,
        ProjectSort::Prompts,
        ProjectSort::Tokens,
        ProjectSort::Cost,
        ProjectSort::Failures,
        ProjectSort::Name,
    ];

    fn label(self) -> &'static str {
        t(match self {
            ProjectSort::Recent => "Recent",
            ProjectSort::Sessions => "Sessions",
            ProjectSort::Prompts => "Prompts",
            ProjectSort::Tokens => "Tokens",
            ProjectSort::Cost => "API value",
            ProjectSort::Failures => "Failures",
            ProjectSort::Name => "Name",
        })
    }
}

/// How the lists are sorted; kept while the window is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Order {
    sessions: SessionSort,
    sessions_desc: bool,
    files: FileSort,
    files_desc: bool,
    projects: ProjectSort,
    projects_desc: bool,
    calls_oldest_first: bool,
    /// The agent map shows all of a session's agents, not only the first few.
    all_agents: bool,
}

impl Default for Order {
    fn default() -> Order {
        Order {
            sessions: SessionSort::Recent,
            sessions_desc: true,
            files: FileSort::Edits,
            files_desc: true,
            projects: ProjectSort::Recent,
            projects_desc: true,
            calls_oldest_first: false,
            all_agents: false,
        }
    }
}

/// A session in the navigation.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub status: Status,
}

/// What the window shows besides the computed statistics.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub dark: bool,
    pub now: i64,
    pub sessions: Vec<Entry>,
    /// Weekly limit readings and the current window's reset, for the limit chart.
    pub history: Vec<forecast::Sample>,
    pub weekly_reset: Option<i64>,
}

/// A list with a line saying what it covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing<T> {
    pub title: String,
    pub items: Vec<T>,
}

/// Computed statistics for the page.
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    Loading,
    Missing,
    Overview(Box<Overview>),
    Session(Box<SessionData>),
    Calls(Box<Listing<CallRow>>),
    Files(Box<Listing<FileRow>>),
    Sessions(Vec<SessionInfo>),
    Projects(Vec<stats::ProjectInfo>),
    Storage(Vec<Storage>),
    /// The latest first.
    Weeks(Vec<Week>),
}

/// What a click asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Compute the data for this page.
    Load(Page),
    /// Open a folder in Explorer.
    Open(PathBuf),
    /// Show a file in Explorer.
    Reveal(PathBuf),
    /// Save CSV text under this file name and show it.
    Export { name: String, csv: String },
    /// Put text on the clipboard.
    Copy(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Action {
    /// Go to a page, remembering this one for "Back".
    Go(Page),
    /// Go to a navigation entry.
    Nav(Page),
    /// Change a filter or period in place.
    Swap(Page),
    Back,
    Open(PathBuf),
    Reveal(PathBuf),
    /// Save what the page lists as CSV.
    Export,
    ClearSearch,
    Copy(String),
    SortSessions(SessionSort),
    SortFiles(FileSort),
    SortProjects(ProjectSort),
    CallsOldestFirst(bool),
    /// Show all of a session's agents, or only the first few.
    AllAgents(bool),
    /// Nothing to click; shows a tooltip.
    Hover,
}

/// Tagged with whether it's in the scrolling page rather than the navigation.
type Hit = crate::hits::Hit<Action, bool>;

struct Tip {
    x: f32,
    y: f32,
    title: String,
    lines: Vec<String>,
}

/// Laid-out clickable areas, areas with tooltips, and the page height.
#[derive(Default)]
struct Layout {
    hits: Vec<Hit>,
    zones: Vec<[f32; 4]>,
    content_h: f32,
}

struct View<'a> {
    m: &'a Model,
    page: &'a Page,
    data: &'a Data,
    period: Period,
    back: bool,
    hover: Option<usize>,
    /// Mouse in window DIPs.
    mouse: Option<(f32, f32)>,
    scroll: f32,
    /// What was typed to search the list.
    search: &'a str,
    order: Order,
    /// What was copied last, so its button can say so.
    copied: Option<&'a str>,
    /// While reading: sessions done, of how many (0 until that's known).
    progress: (u32, u32),
}

struct Ui<'a> {
    c: Option<&'a Canvas<'a>>,
    g: std::rc::Rc<gfx::Gfx>,
    pal: &'static Palette,
    hover: Option<usize>,
    /// Mouse in page coordinates, when over the page.
    mouse: Option<(f32, f32)>,
    scroll: f32,
    scrolls: Cell<bool>,
    hits: RefCell<Vec<Hit>>,
    zones: RefCell<Vec<[f32; 4]>>,
    tip: RefCell<Option<Tip>>,
}

/// One bar of a bar chart.
struct Bar {
    label: String,
    value: f64,
    title: String,
    lines: Vec<String>,
}

impl Ui<'_> {
    fn text(&self, s: &str, size: f32, bold: bool, max: Option<f32>) -> Result<gfx::Text> {
        self.g.text(s, size, if bold { SEMIBOLD } else { NORMAL }, max)
    }

    /// Registers a clickable area; returns whether the mouse is on it.
    fn hit(&self, x: f32, y: f32, w: f32, h: f32, action: Action) -> bool {
        let mut hits = self.hits.borrow_mut();
        hits.push(Hit { x, y, w, h, action, tag: self.scrolls.get() });
        self.hover == Some(hits.len() - 1)
    }

    /// Registers an area that follows the mouse; returns the mouse if on it.
    fn zone(&self, x: f32, y: f32, w: f32, h: f32) -> Option<(f32, f32)> {
        self.zones.borrow_mut().push([x, y, w, h]);
        self.mouse.filter(|&(mx, my)| mx >= x && mx < x + w && my >= y && my < y + h)
    }

    /// Whether something at `y` with height `h` is drawn now (on screen).
    fn shown(&self, y: f32, h: f32) -> bool {
        self.c.is_some() && (!self.scrolls.get() || (y + h >= self.scroll && y <= self.scroll + view_h()))
    }

    fn tip(&self, x: f32, y: f32, title: String, lines: Vec<String>) {
        if self.c.is_some() {
            *self.tip.borrow_mut() = Some(Tip { x, y, title, lines });
        }
    }

    fn draw_tip(&self) -> Result<()> {
        let Some(c) = self.c else { return Ok(()) };
        let Some(tip) = self.tip.borrow_mut().take() else { return Ok(()) };
        let max = 380.0;
        let title = self.g.wrapped(&tip.title, 12.0, SEMIBOLD, max)?;
        let lines = tip.lines.iter().map(|l| self.g.wrapped(l, 12.0, NORMAL, max)).collect::<Result<Vec<_>>>()?;
        let tw = lines.iter().map(|l| l.width).fold(title.width, f32::max) + 24.0;
        let th = title.height + lines.iter().map(|l| l.height + 2.0).sum::<f32>() + 18.0;
        let mut x = tip.x + 14.0;
        if x + tw > WIDTH - 8.0 {
            x = (tip.x - tw - 14.0).max(NAV_W + 4.0);
        }
        let mut y = tip.y - th - 10.0;
        if y < self.scroll + 6.0 {
            y = tip.y + 18.0;
        }
        let pal = self.pal;
        c.fill_round_rect(x, y + 3.0, tw, th, 8.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.22 });
        c.fill_round_rect(x, y, tw, th, 8.0, pal.card_hover);
        c.stroke_round_rect(x + 0.5, y + 0.5, tw - 1.0, th - 1.0, 8.0, 1.0, pal.divider.with_alpha(2.0));
        c.text(&title, x + 12.0, y + 9.0, pal.text);
        let mut ly = y + 9.0 + title.height + 2.0;
        for l in &lines {
            c.text(l, x + 12.0, ly, pal.text_dim);
            ly += l.height + 2.0;
        }
        Ok(())
    }

    /// A rounded card with a title, an optional note and an optional link at
    /// the right; returns the y where its content starts.
    #[allow(clippy::too_many_arguments, reason = "position, size, and three texts")]
    fn card(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        title: &str,
        note: &str,
        link: Option<(&str, Action)>,
    ) -> Result<f32> {
        let t = self.text(title, 13.0, true, None)?;
        let mut right = x + w - 16.0;
        if let Some((label, action)) = link {
            let l = self.text(label, 12.0, false, None)?;
            let chev = self.g.glyph(CHEVRON, 9.0)?;
            let lw = l.width + 4.0 + chev.width;
            let hovered = self.hit(right - lw - 6.0, y + 6.0, lw + 12.0, t.height + 12.0, action);
            if let Some(c) = self.c {
                c.fill_round_rect(x, y, w, h, 8.0, self.pal.card);
                if hovered {
                    c.fill_round_rect(right - lw - 6.0, y + 7.0, lw + 12.0, t.height + 10.0, 5.0, self.pal.row_hover);
                }
                let color = if hovered { self.pal.text } else { self.pal.accent };
                c.text(&l, right - lw, y + 12.0 + (t.height - l.height) / 2.0, color);
                c.text(&chev, right - chev.width, y + 12.0 + (t.height - chev.height) / 2.0 + 1.0, color);
            }
            right -= lw + 16.0;
        } else if let Some(c) = self.c {
            c.fill_round_rect(x, y, w, h, 8.0, self.pal.card);
        }
        if let Some(c) = self.c {
            c.text(&t, x + 16.0, y + 12.0, self.pal.text);
            if !note.is_empty() {
                let n = self.text(note, 12.0, false, Some((right - x - 16.0 - t.width - 16.0).max(10.0)))?;
                c.text(&n, right - n.width, y + 12.0 + (t.height - n.height) / 2.0, self.pal.text_dim);
            }
        }
        Ok(y + 12.0 + t.height + 10.0)
    }

    /// A tile with a big number and a label; clickable with an action,
    /// explained in a tooltip with a tip.
    fn tile(&self, x: f32, y: f32, w: f32, h: f32, t: &Tile) -> Result<()> {
        let clickable = t.action.is_some();
        let explained = t.tip.is_some() || t.change.is_some();
        let hovered = match (&t.action, explained) {
            (Some(a), _) => self.hit(x, y, w, h, a.clone()),
            (None, true) => self.hit(x, y, w, h, Action::Hover),
            _ => false,
        };
        let Some(c) = self.c else { return Ok(()) };
        let tx = x + if t.accent.is_some() { 24.0 } else { 16.0 };
        // A clickable tile's chevron sits on the number's row, which is short: the
        // label below keeps the tile's width.
        let chev = if clickable { Some(self.g.glyph(CHEVRON, 9.0)?) } else { None };
        let value_w = chev.as_ref().map_or(w - 30.0, |chev| x + w - 12.0 - chev.width - 8.0 - tx);
        let v = self.text(&t.value, 22.0, true, Some(value_w.max(10.0)))?;
        let l = self.text(&t.label, 12.0, false, Some(w - 30.0))?;
        c.fill_round_rect(x, y, w, h, 8.0, if hovered { self.pal.card_hover } else { self.pal.card });
        if let Some(a) = t.accent {
            c.fill_round_rect(x + 12.0, y + 12.0, 3.0, h - 24.0, 1.5, a);
        }
        c.text(&v, tx, y + 12.0, self.pal.text);
        c.text(&l, tx, y + 14.0 + v.height, if clickable { self.pal.accent } else { self.pal.text_dim });
        if let Some((pct, _)) = &t.change {
            // Neutral colour: more isn't better or worse by itself.
            let text = match *pct {
                p if p.abs() < 1.0 => i18n::t("= same as before").to_string(),
                p => format!("{} {:.0}%", if p > 0.0 { "▲" } else { "▼" }, p.abs().min(999.0)),
            };
            let d = self.text(&text, 11.0, false, Some(w - 30.0))?;
            c.text(&d, tx, y + 16.0 + v.height + l.height, self.pal.text_dim.with_alpha(0.85));
        }
        if let Some(chev) = chev {
            let color = if hovered { self.pal.text } else { self.pal.text_dim.with_alpha(0.7) };
            c.text(&chev, x + w - 12.0 - chev.width, y + 12.0 + (v.height - chev.height) / 2.0 + 1.0, color);
        }
        if hovered && explained {
            let mut lines: Vec<String> = t.tip.iter().cloned().collect();
            if let Some((pct, what)) = &t.change {
                let pct_text = format!("{:.0}", pct.abs());
                lines.push(match *pct {
                    p if p.abs() < 1.0 => f("About the same as {0}.", &[what]),
                    p if p > 0.0 => f("{0}% more than {1}.", &[&pct_text, what]),
                    _ => f("{0}% less than {1}.", &[&pct_text, what]),
                });
            }
            self.tip(x + w / 2.0, y + 6.0, format!("{} {}", t.value, t.label), lines);
        }
        Ok(())
    }

    /// Vertical bars with labels underneath; the hovered bar shows its tooltip.
    fn bars(&self, x: f32, y: f32, w: f32, h: f32, items: &[Bar], highlight: Option<usize>) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mouse = self.zone(x, y, w, h);
        let Some(c) = self.c else { return Ok(()) };
        let max = items.iter().map(|b| b.value).fold(0.0, f64::max).max(1.0);
        let (label_h, value_h) = (18.0, 16.0);
        let chart_h = h - label_h - value_h;
        let step = w / items.len() as f32;
        let bar_w = (step * 0.56).clamp(2.0, 40.0);
        let hovered = mouse.map(|(mx, _)| (((mx - x) / step) as usize).min(items.len() - 1));
        let mut tip = None;
        for (i, b) in items.iter().enumerate() {
            let cx = x + step * (i as f32 + 0.5);
            let bh = (chart_h * (b.value / max) as f32).max(if b.value > 0.0 { 3.0 } else { 0.0 });
            let top = y + value_h + chart_h - bh;
            let over = hovered == Some(i);
            let lit = over || highlight == Some(i);
            if over {
                c.fill_round_rect(cx - step / 2.0, y, step, value_h + chart_h + 4.0, 5.0, self.pal.row_hover);
                tip = Some((cx, top, b));
            }
            c.fill_round_rect(
                cx - bar_w / 2.0,
                y + value_h,
                bar_w,
                chart_h,
                4.0f32.min(bar_w / 2.0),
                self.pal.track.with_alpha(0.5),
            );
            if bh > 0.0 {
                let color = if lit { self.pal.accent } else { self.pal.accent.with_alpha(0.55) };
                c.fill_round_rect(cx - bar_w / 2.0, top, bar_w, bh, 4.0f32.min(bh / 2.0).min(bar_w / 2.0), color);
            }
            if b.value > 0.0 && items.len() <= 14 {
                let t = self.text(&tokens(b.value as u64), 11.0, lit, None)?;
                c.text(
                    &t,
                    cx - t.width / 2.0,
                    top - t.height - 1.0,
                    if lit { self.pal.text } else { self.pal.text_dim },
                );
            }
            if !b.label.is_empty() {
                let t = self.text(&b.label, 11.0, lit, None)?;
                let tx = (cx - t.width / 2.0).clamp(x - 6.0, x + w + 6.0 - t.width);
                c.text(&t, tx, y + h - label_h + 3.0, if lit { self.pal.text } else { self.pal.text_dim });
            }
        }
        if let Some((cx, top, b)) = tip {
            self.tip(cx, top, b.title.clone(), b.lines.clone());
        }
        Ok(())
    }

    /// A ranked list: name, a bar for the share of the largest, and the value.
    /// Rows with an action are clickable.
    #[allow(clippy::too_many_arguments, reason = "position, width, items, rows, and two closures")]
    fn ranked(
        &self,
        x: f32,
        y: f32,
        w: f32,
        items: &[(String, f64)],
        rows: usize,
        fmt: impl Fn(f64) -> String,
        action: impl Fn(&str) -> Option<Action>,
    ) -> Result<f32> {
        let max = items.first().map_or(1.0, |(_, v)| v.max(1.0));
        let mut y = y;
        if items.is_empty() {
            if let Some(c) = self.c {
                let t = self.text(t("Nothing yet."), 12.0, false, None)?;
                c.text(&t, x, y, self.pal.text_dim);
            }
            return Ok(y + 22.0);
        }
        // One track length for every row (up to the widest number), so the bars compare.
        let mut value_w = 0.0f32;
        if self.c.is_some() {
            for (_, v) in items.iter().take(rows) {
                value_w = value_w.max(self.text(&fmt(*v), 12.0, true, None)?.width);
            }
        }
        for (name, v) in items.iter().take(rows) {
            let hovered = action(name).is_some_and(|a| self.hit(x - 8.0, y - 3.0, w + 16.0, 22.0, a));
            if let Some(c) = self.c {
                let value = self.text(&fmt(*v), 12.0, true, None)?;
                let name_t = self.text(name, 12.0, false, Some(w * 0.42))?;
                if hovered {
                    c.fill_round_rect(x - 8.0, y - 3.0, w + 16.0, 22.0, 5.0, self.pal.row_hover);
                }
                let bar_x = x + w * 0.45;
                let bar_w = w - (w * 0.45) - value_w - 10.0;
                c.text(&name_t, x, y, if hovered { self.pal.accent } else { self.pal.text });
                let mid = y + name_t.height / 2.0;
                c.fill_round_rect(bar_x, mid - 3.0, bar_w, 6.0, 3.0, self.pal.track.with_alpha(0.6));
                c.fill_round_rect(bar_x, mid - 3.0, (bar_w * (*v / max) as f32).max(6.0), 6.0, 3.0, self.pal.accent);
                c.text(&value, x + w - value.width, y, self.pal.text_dim);
            }
            y += 22.0;
        }
        Ok(y)
    }

    /// An area chart of `raw` points over `span` (x) and 0..`y_max`; the
    /// point nearest the mouse shows its tooltip.
    #[allow(clippy::too_many_arguments, reason = "position, size, data, scales, colour, tooltip")]
    fn area_chart(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        raw: &[(i64, f64)],
        span: (i64, i64),
        y_max: f64,
        color: Color,
        tip: &dyn Fn(i64, f64) -> (String, Vec<String>),
    ) {
        let mouse = self.zone(x - 4.0, y, w + 8.0, h);
        let Some(c) = self.c else { return };
        for i in 1..4 {
            c.fill_rect(x, y + h * i as f32 / 4.0, w, 1.0, self.pal.divider);
        }
        if raw.is_empty() {
            return;
        }
        let pts: Vec<(f32, f32)> =
            normalize(raw, span.0, span.1, y_max).iter().map(|&(px, py)| (x + px * w, y + h - py * h)).collect();
        if pts.len() >= 2 {
            c.area(&pts, y + h, y, color.with_alpha(0.35), color.with_alpha(0.0));
            c.polyline(&pts, 1.8, color);
        }
        if let Some((mx, _)) = mouse {
            let width = (span.1 - span.0).max(1);
            let t = span.0 + ((mx - x) / w * width as f32) as i64;
            if let Some(&(pt, pv)) = raw.iter().min_by_key(|(at, _)| (at - t).abs()) {
                let px = x + w * ((pt - span.0) as f32 / width as f32).clamp(0.0, 1.0);
                let py = y + h - h * (pv / y_max.max(1e-9)).clamp(0.0, 1.0) as f32;
                c.fill_rect(px - 0.5, y, 1.0, h, self.pal.text_dim.with_alpha(0.5));
                c.fill_circle(px, py, 5.5, self.pal.card);
                c.fill_circle(px, py, 3.5, color);
                let (title, lines) = tip(pt, pv);
                self.tip(px, py, title, lines);
            }
        }
    }

    /// Prompts and replies by weekday and hour; returns the y below it.
    fn heatmap(&self, x: f32, y: f32, w: f32, heat: &[[u32; 24]]) -> Result<f32> {
        let (label_w, gap, cell_h) = (40.0, 3.0, 18.0);
        let cell_w = (w - label_w - gap * 23.0) / 24.0;
        let grid_h = 7.0 * (cell_h + gap);
        let mouse = self.zone(x + label_w, y, w - label_w, grid_h);
        let bottom = y + grid_h + 18.0;
        let Some(c) = self.c else { return Ok(bottom) };
        let max = heat.iter().flatten().copied().max().unwrap_or(0).max(1);
        let cell = mouse.map(|(mx, my)| {
            ((((my - y) / (cell_h + gap)) as usize).min(6), (((mx - x - label_w) / (cell_w + gap)) as usize).min(23))
        });
        for (d, row) in heat.iter().enumerate().take(7) {
            let cy = y + d as f32 * (cell_h + gap);
            let name = self.text(&timefmt::weekday(A_MONDAY + d as i64 * DAY), 11.0, false, None)?;
            c.text(&name, x, cy + (cell_h - name.height) / 2.0, self.pal.text_dim);
            for (hr, &v) in row.iter().enumerate() {
                let cx = x + label_w + hr as f32 * (cell_w + gap);
                let color = if v == 0 {
                    self.pal.track.with_alpha(0.35)
                } else {
                    self.pal.accent.with_alpha(0.18 + 0.82 * (v as f32 / max as f32))
                };
                c.fill_round_rect(cx, cy, cell_w, cell_h, 3.0, color);
                if cell == Some((d, hr)) {
                    c.stroke_round_rect(cx - 1.0, cy - 1.0, cell_w + 2.0, cell_h + 2.0, 4.0, 1.5, self.pal.text);
                }
            }
        }
        for hr in (0..24).step_by(3) {
            let t = self.text(&format!("{hr}:00"), 11.0, false, None)?;
            c.text(&t, x + label_w + hr as f32 * (cell_w + gap), y + grid_h + 2.0, self.pal.text_dim);
        }
        if let Some((d, hr)) = cell {
            let v = heat[d][hr];
            let day = timefmt::weekday(A_MONDAY + d as i64 * DAY);
            let cx = x + label_w + hr as f32 * (cell_w + gap) + cell_w / 2.0;
            let what = match v {
                0 => t("Nothing").to_string(),
                1 => t("1 prompt or reply").to_string(),
                n => f("{0} prompts and replies", &[&n]),
            };
            self.tip(cx, y + d as f32 * (cell_h + gap), format!("{day} {hr}:00–{}:00", hr + 1), vec![what]);
        }
        Ok(bottom)
    }

    /// A pill button ending at `right`; returns its width.
    fn button(&self, right: f32, y: f32, glyph: char, label: &str, action: Action) -> Result<f32> {
        let t = self.text(label, 12.0, false, None)?;
        let g = self.g.glyph(glyph, 13.0)?;
        let w = 12.0 + g.width + 6.0 + t.width + 12.0;
        let h = 30.0;
        let x = right - w;
        let hovered = self.hit(x, y, w, h, action);
        if let Some(c) = self.c {
            c.fill_round_rect(x, y, w, h, 6.0, if hovered { self.pal.card_hover } else { self.pal.card });
            c.stroke_round_rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, 6.0, 1.0, self.pal.divider);
            c.text(&g, x + 12.0, y + (h - g.height) / 2.0 + 1.0, self.pal.text);
            c.text(&t, x + 12.0 + g.width + 6.0, y + (h - t.height) / 2.0, self.pal.text);
        }
        Ok(w)
    }

    /// The search field of list pages, ending at `right`; typing anywhere
    /// goes into it. Returns its width.
    fn search_box(&self, right: f32, y: f32, text: &str) -> Result<f32> {
        let (w, h) = (220.0, 30.0);
        let x = right - w;
        let clear = (!text.is_empty()).then(|| self.hit(right - 30.0, y, 30.0, h, Action::ClearSearch));
        if let Some(c) = self.c {
            let pal = self.pal;
            c.fill_round_rect(x, y, w, h, 6.0, pal.card);
            let stroke = if text.is_empty() { pal.divider } else { pal.accent };
            c.stroke_round_rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, 6.0, 1.0, stroke);
            let g = self.g.glyph(SEARCH, 12.0)?;
            c.text(&g, x + 10.0, y + (h - g.height) / 2.0 + 1.0, pal.text_dim);
            let tx = x + 10.0 + g.width + 8.0;
            if text.is_empty() {
                let t = self.text(t("Type to search"), 12.0, false, None)?;
                c.text(&t, tx, y + (h - t.height) / 2.0, pal.text_dim);
            } else {
                let t = self.text(text, 12.0, false, Some(w - (tx - x) - 36.0))?;
                c.text(&t, tx, y + (h - t.height) / 2.0, pal.text);
                c.fill_rect(tx + t.width + 1.0, y + 8.0, 1.0, h - 16.0, pal.text);
                let x_mark = self.text("✕", 11.0, false, None)?;
                let color = if clear == Some(true) { pal.text } else { pal.text_dim };
                c.text(&x_mark, right - 15.0 - x_mark.width / 2.0, y + (h - x_mark.height) / 2.0, color);
            }
        }
        Ok(w)
    }

    /// "Sort by" and small pills; returns the y below them.
    fn sort_row(&self, y: f32, label: &str, items: &[(String, bool, Action)]) -> Result<f32> {
        let h = 26.0;
        let l = self.text(label, 12.0, false, None)?;
        if let Some(c) = self.c {
            c.text(&l, X0, y + (h - l.height) / 2.0, self.pal.text_dim);
        }
        let mut x = X0 + l.width + 12.0;
        for (text, selected, action) in items {
            let t = self.text(text, 12.0, *selected, None)?;
            let w = t.width + 20.0;
            let hovered = self.hit(x, y, w, h, action.clone());
            if let Some(c) = self.c {
                let pal = self.pal;
                if *selected {
                    c.fill_round_rect(x, y, w, h, 6.0, pal.accent.with_alpha(0.18));
                    c.stroke_round_rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, 6.0, 1.0, pal.accent);
                } else if hovered {
                    c.fill_round_rect(x, y, w, h, 6.0, pal.row_hover.with_alpha(1.5));
                }
                let fg = if *selected { pal.accent } else { pal.text };
                c.text(&t, x + 10.0, y + (h - t.height) / 2.0, fg);
            }
            x += w + 4.0;
        }
        Ok(y + h + GAP)
    }

    /// Filter chips, wrapping; returns the y below them.
    fn chips(&self, x: f32, y: f32, w: f32, items: &[(String, bool, Action)]) -> Result<f32> {
        let (h, gap) = (28.0, 8.0);
        let (mut cx, mut cy) = (x, y);
        for (label, selected, action) in items {
            let t = self.text(label, 12.0, *selected, None)?;
            let cw = t.width + 24.0;
            if cx + cw > x + w && cx > x {
                cx = x;
                cy += h + gap;
            }
            let hovered = self.hit(cx, cy, cw, h, action.clone());
            if let Some(c) = self.c {
                let (bg, fg) = match (selected, hovered) {
                    (true, _) => (self.pal.accent, self.pal.on_accent),
                    (false, true) => (self.pal.card_hover, self.pal.text),
                    (false, false) => (self.pal.card, self.pal.text),
                };
                c.fill_round_rect(cx, cy, cw, h, h / 2.0, bg);
                c.text(&t, cx + 12.0, cy + (h - t.height) / 2.0, fg);
            }
            cx += cw + gap;
        }
        Ok(cy + h + GAP)
    }

    /// A segmented control ending at `right`.
    fn segmented(&self, right: f32, y: f32, items: &[(&str, bool, Action)]) -> Result<()> {
        let texts = items.iter().map(|(l, s, _)| self.text(l, 12.0, *s, None)).collect::<Result<Vec<_>>>()?;
        let total: f32 = texts.iter().map(|t| t.width + 24.0).sum::<f32>() + 6.0;
        let h = 30.0;
        let mut x = right - total;
        if let Some(c) = self.c {
            c.fill_round_rect(x, y, total, h, h / 2.0, self.pal.card);
        }
        x += 3.0;
        for ((_, selected, action), t) in items.iter().zip(&texts) {
            let sw = t.width + 24.0;
            let hovered = self.hit(x, y + 3.0, sw, h - 6.0, action.clone());
            if let Some(c) = self.c {
                if *selected {
                    c.fill_round_rect(x, y + 3.0, sw, h - 6.0, (h - 6.0) / 2.0, self.pal.accent);
                } else if hovered {
                    c.fill_round_rect(x, y + 3.0, sw, h - 6.0, (h - 6.0) / 2.0, self.pal.row_hover.with_alpha(1.5));
                }
                let fg = if *selected { self.pal.on_accent } else { self.pal.text };
                c.text(t, x + 12.0, y + (h - t.height) / 2.0, fg);
            }
            x += sw;
        }
        Ok(())
    }
}

/// Fits `points` into 0..1 on both axes; `y_max` is the top of the scale.
fn normalize(points: &[(i64, f64)], x_min: i64, x_max: i64, y_max: f64) -> Vec<(f32, f32)> {
    let span = (x_max - x_min).max(1) as f64;
    // At most a few hundred points: more don't show at this size.
    let step = (points.len() / 300).max(1);
    points
        .iter()
        .step_by(step)
        .map(|&(t, v)| ((((t - x_min) as f64) / span) as f32, (v / y_max.max(1e-9)).clamp(0.0, 1.0) as f32))
        .collect()
}

fn duration(secs: i64) -> String {
    let m = secs.max(0) / 60;
    match (m / 1440, (m % 1440) / 60, m % 60) {
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m:02}m"),
        (d, h, _) => f("{0}d {1}h", &[&d, &h]),
    }
}

fn percent(v: Option<f64>) -> String {
    v.map_or("–".into(), |v| format!("{:.0}%", v * 100.0))
}

/// A count in at most five characters.
fn num(n: u64) -> String {
    if n < 10_000 { n.to_string() } else { tokens(n) }
}

/// `n` in the template `one` or `many`, e.g. `"{0} file"` and `"{0} files"`.
fn plural(n: u64, one: &'static str, many: &'static str) -> String {
    f(if n == 1 { one } else { many }, &[&num(n)])
}

/// A point in time, with as much date as it needs.
fn when(t: i64, now: i64) -> String {
    if now - t > 6 * DAY {
        format!("{} {}", timefmt::local_date(t), timefmt::local_time(t, false))
    } else {
        timefmt::local_time(t, now - t > 20 * HOUR)
    }
}

/// `text` as one word of a command to paste into a shell: in single quotes,
/// where a shell expands nothing (in double quotes, a folder called `$(…)` would
/// run). A quote in it is written the shell's way: doubled for PowerShell, closed
/// and escaped for the shell in a WSL distribution (`wsl`). A word of plain characters stays as it is.
fn shell_word(text: &str, wsl: bool) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./".contains(c) || (!wsl && (c == ':' || c == '\\'));
    if !text.is_empty() && text.chars().all(plain) {
        return text.to_string();
    }
    let mut out = String::from("'");
    for c in text.chars() {
        match c {
            '\'' if wsl => out.push_str(r"'\''"),
            // PowerShell reads the typographic quotes as quotes too.
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' if !wsl => {
                out.push(c);
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Longest text the search takes.
const SEARCH_CHARS: usize = 200;

/// A path as Windows can open it (Linux paths of WSL sessions included).
fn local_path(path: &str, distro: Option<&String>) -> PathBuf {
    match distro {
        Some(d) if path.starts_with('/') => crate::registry::wsl_path(d, path),
        _ => PathBuf::from(path),
    }
}

fn file_name(path: &str) -> &str {
    path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    let p = path.trim_end_matches(['\\', '/']);
    p.rsplit_once(['\\', '/']).map_or("", |(folder, _)| folder)
}

fn bucket_title(t: i64, bucket: i64) -> String {
    let mid = t + 12 * HOUR;
    match bucket {
        HOUR => format!("{} – {}", timefmt::local_time(t, true), timefmt::local_time(t + HOUR, false)),
        DAY => format!("{} {}", timefmt::weekday(mid), timefmt::local_date(mid)),
        _ => f("Week of {0}", &[&timefmt::local_date(mid)]),
    }
}

fn bucket_label(t: i64, bucket: i64, i: usize, n: usize) -> String {
    let mid = t + 12 * HOUR;
    let every = |k: usize| (n - 1 - i) % k.max(1) == 0;
    match bucket {
        HOUR if i % 3 == 0 => timefmt::local_time(t, false),
        DAY if n <= 7 => timefmt::weekday(mid),
        DAY if every(n.div_ceil(7)) => timefmt::local_date(mid),
        b if b > DAY && every(n.div_ceil(7)) => timefmt::local_date(mid),
        _ => String::new(),
    }
}

fn activity_lines(a: &Activity) -> Vec<String> {
    let mut lines = vec![if a.agent_output > 0 {
        f("{0} tokens written ({1} by agents)", &[&tokens(a.output + a.agent_output), &tokens(a.agent_output)])
    } else {
        f("{0} tokens written", &[&tokens(a.output)])
    }];
    lines.push(format!(
        "{} · {}",
        plural(a.prompts.into(), "{0} prompt", "{0} prompts"),
        plural(a.replies.into(), "{0} reply", "{0} replies")
    ));
    if a.tool_calls > 0 {
        let failed = if a.errors > 0 { format!(" · {}", f("{0} failed", &[&a.errors])) } else { String::new() };
        lines.push(format!("{}{failed}", plural(a.tool_calls.into(), "{0} tool call", "{0} tool calls")));
    }
    if a.cost > 0.0 {
        lines.push(f("≈ {0} at API prices", &[&stats::dollars(a.cost)]));
    }
    lines
}

/// A turn's length: `45s`, `3m 10s`, `1h 05m`.
fn turn_time(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => duration(s),
    }
}

/// A turn in a list; with a session, clicking opens it.
fn turn_row(ui: &Ui, now: i64, x: f32, y: f32, w: f32, t: &Turn, session: Option<(&str, &str)>) -> Result<()> {
    let h = 44.0;
    let action = session.map_or(Action::Hover, |(id, _)| Action::Go(Page::Session(id.to_string())));
    let hovered = ui.hit(x, y, w, h, action);
    if !ui.shown(y, h) {
        return Ok(());
    }
    let Some(c) = ui.c else { return Ok(()) };
    let pal = ui.pal;
    if hovered {
        c.fill_round_rect(x, y, w, h, 6.0, pal.row_hover);
    }
    let d = ui.text(&turn_time(t.secs), 14.0, true, None)?;
    c.text(&d, x + 10.0, y + (h - d.height) / 2.0, pal.text);
    let tx = x + 100.0;
    let prompt = ui.text(&t.prompt, 13.0, false, Some(x + w - 10.0 - tx))?;
    c.text(&prompt, tx, y + 5.0, if hovered { pal.accent } else { pal.text });
    let mut sub = when(t.at, now);
    if let Some((_, title)) = session {
        sub = format!("{sub} · {title}");
    }
    let sub = ui.text(&sub, 12.0, false, Some(x + w - 10.0 - tx))?;
    c.text(&sub, tx, y + 7.0 + prompt.height, pal.text_dim);
    if hovered {
        let mx = ui.mouse.map_or(tx, |(mx, _)| mx);
        ui.tip(mx, y + 2.0, f("Turn of {0}", &[&turn_time(t.secs)]), vec![t.prompt.clone()]);
    }
    Ok(())
}

/// A kind of failure with how often it happened.
#[allow(clippy::too_many_arguments, reason = "position, width, data, state, action")]
fn failure_row(ui: &Ui, now: i64, x: f32, y: f32, w: f32, f: &Failure, selected: bool, action: Action) -> Result<()> {
    let h = 44.0;
    let hovered = ui.hit(x, y, w, h, action);
    if !ui.shown(y, h) {
        return Ok(());
    }
    let Some(c) = ui.c else { return Ok(()) };
    let pal = ui.pal;
    if selected {
        c.fill_round_rect(x, y, w, h, 6.0, pal.crit.with_alpha(0.14));
    } else if hovered {
        c.fill_round_rect(x, y, w, h, 6.0, pal.row_hover);
    }
    let n = ui.text(&format!("{}×", num(f.count.into())), 14.0, true, None)?;
    c.text(&n, x + 10.0, y + (h - n.height) / 2.0, pal.crit);
    let tx = x + 70.0;
    let sig = ui.text(&f.signature, 13.0, false, Some(x + w - 10.0 - tx))?;
    c.text(&sig, tx, y + 5.0, if hovered { pal.accent } else { pal.text });
    let sub = ui.text(
        &format!("{} · {}", i18n::f("last {0}", &[&when(f.example.at, now)]), f.example.detail),
        12.0,
        false,
        Some(x + w - 10.0 - tx),
    )?;
    c.text(&sub, tx, y + 7.0 + sig.height, pal.text_dim);
    if hovered {
        let mut lines = vec![f.example.detail.clone()];
        if let Some(e) = &f.example.error {
            lines.push(i18n::f("Error: {0}", &[e]));
        }
        let mx = ui.mouse.map_or(tx, |(mx, _)| mx);
        ui.tip(mx, y + 2.0, format!("{}× {}", f.count, f.signature), lines);
    }
    Ok(())
}

/// A card listing turns; returns the y below it.
fn turns_card(ui: &Ui, v: &View, y: f32, rows: &[(&Turn, Option<(&str, &str)>)]) -> Result<f32> {
    if rows.is_empty() {
        return Ok(y);
    }
    let h = 12.0 + 18.0 + 10.0 + rows.len() as f32 * 46.0 + 8.0;
    let cy = ui.card(X0, y, W, h, t("Longest turns"), t("from a prompt to its last step"), None)?;
    let mut ry = cy - 4.0;
    for (t, session) in rows {
        turn_row(ui, v.m.now, X0 + 6.0, ry, W - 12.0, t, *session)?;
        ry += 46.0;
    }
    Ok(y + h + GAP)
}

/// A card with the most frequent failures; returns the y below it.
fn failures_card(ui: &Ui, v: &View, y: f32, list: &[Failure], page: &dyn Fn(Filter) -> Page) -> Result<f32> {
    if list.is_empty() {
        return Ok(y);
    }
    let h = 12.0 + 18.0 + 10.0 + list.len() as f32 * 46.0 + 8.0;
    let all = Action::Go(page(Filter::Failed));
    let cy = ui.card(X0, y, W, h, t("Common failures"), t("alike errors grouped"), Some((t("All failures"), all)))?;
    let mut ry = cy - 4.0;
    for f in list {
        let action = Action::Go(page(Filter::Error(f.signature.clone())));
        failure_row(ui, v.m.now, X0 + 6.0, ry, W - 12.0, f, false, action)?;
        ry += 46.0;
    }
    Ok(y + h + GAP)
}

/// What a folder or file of Claude Code's own is for, if it's a known one.
fn storage_label(name: &str) -> Option<&'static str> {
    Some(t(match name {
        "projects" => "Transcripts",
        "file-history" => "File history (undo)",
        "shell-snapshots" => "Shell snapshots",
        "session-env" => "Session environments",
        "plugins" => "Plugins",
        "skills" => "Skills",
        "todos" => "To-do lists",
        "plans" => "Plans",
        "cache" => "Cache",
        "backups" => "Backups",
        "debug" => "Debug logs",
        "statsig" => "Feature flags",
        "ide" => "IDE connections",
        "sessions" => "Running sessions",
        "history.jsonl" => "Prompt history",
        "settings.json" => "Settings",
        _ => return None,
    }))
}

/// Lays out (and with a canvas draws) the window.
fn render<'a>(c: Option<&'a Canvas<'a>>, v: &View) -> Result<Layout> {
    let page_mouse = v.mouse.filter(|&(x, _)| x >= NAV_W).map(|(x, y)| (x, y + v.scroll));
    let ui = Ui {
        c,
        g: gfx::get()?,
        pal: theme::palette(v.m.dark),
        hover: v.hover,
        mouse: page_mouse,
        scroll: v.scroll,
        scrolls: Cell::new(false),
        hits: RefCell::new(Vec::new()),
        zones: RefCell::new(Vec::new()),
        tip: RefCell::new(None),
    };
    nav(&ui, v)?;
    ui.scrolls.set(true);
    if let Some(c) = c {
        c.push_clip(NAV_W, 0.0, WIDTH - NAV_W, view_h());
        c.set_offset(0.0, -v.scroll);
    }
    let bottom = page(&ui, v)?;
    ui.draw_tip()?;
    let content_h = bottom + PAD;
    if let Some(c) = c {
        c.set_offset(0.0, 0.0);
        c.pop_clip();
        let view = view_h();
        if content_h > view {
            let track = view - 16.0;
            let thumb = (track * view / content_h).max(40.0);
            let top = 8.0 + (track - thumb) * (v.scroll / (content_h - view)).clamp(0.0, 1.0);
            c.fill_round_rect(WIDTH - 9.0, top, 4.0, thumb, 2.0, ui.pal.text_dim.with_alpha(0.45));
        }
    }
    Ok(Layout { hits: ui.hits.into_inner(), zones: ui.zones.into_inner(), content_h })
}

fn nav(ui: &Ui, v: &View) -> Result<()> {
    let pal = ui.pal;
    let title = ui.text(t("Statistics"), 14.0, true, None)?;
    if let Some(c) = ui.c {
        c.text(&title, PAD, PAD - 4.0, pal.text);
    }
    let nav_w = NAV_W - PAD;
    let item = |y: f32, h: f32, target: Page, draw: &dyn Fn(&Canvas, f32, f32)| {
        let selected = v.page.under(&target);
        let hovered = ui.hit(PAD / 2.0, y, nav_w, h, Action::Nav(target));
        if let Some(c) = ui.c {
            if selected {
                c.fill_round_rect(PAD / 2.0, y, nav_w, h, 5.0, pal.row_hover.with_alpha(1.6));
                c.fill_round_rect(PAD / 2.0, y + h / 2.0 - 8.0, 3.0, 16.0, 1.5, pal.accent);
            } else if hovered {
                c.fill_round_rect(PAD / 2.0, y, nav_w, h, 5.0, pal.row_hover);
            }
            draw(c, y, h);
        }
    };
    let mut y = PAD + 30.0;
    for (glyph, label, target) in [
        (CHART, "Overview", Page::Overview(v.period)),
        (LIST, "All sessions", Page::Sessions { archived: None }),
        (FOLDER, "Projects", Page::Projects),
        (CALENDAR, "Weeks", Page::Weeks),
        (DRIVE, "Storage", Page::Storage),
    ] {
        let icon = ui.g.glyph(glyph, 16.0)?;
        let label = ui.text(t(label), 13.0, false, None)?;
        item(y, 38.0, target, &|c, y, h| {
            c.text(&icon, PAD / 2.0 + 14.0, y + (h - icon.height) / 2.0, pal.text);
            c.text(&label, PAD / 2.0 + 42.0, y + (h - label.height) / 2.0, pal.text);
        });
        y += 40.0;
    }
    y += 12.0;
    if v.m.sessions.is_empty() {
        return Ok(());
    }
    let header = ui.text(t("Active sessions"), 12.0, true, None)?;
    if let Some(c) = ui.c {
        c.text(&header, PAD, y, pal.text_dim);
    }
    y += header.height + 6.0;
    for e in v.m.sessions.iter().take(NAV_SESSIONS) {
        let t = ui.text(&e.title, 13.0, false, Some(nav_w - 40.0))?;
        let s = ui.text(&e.workspace, 11.5, false, Some(nav_w - 40.0))?;
        let color = pal.status(e.status);
        item(y, 44.0, Page::Session(e.id.clone()), &|c, y, _| {
            c.fill_circle(PAD / 2.0 + 18.0, y + 13.0, 3.5, color);
            c.text(&t, PAD / 2.0 + 30.0, y + 5.0, pal.text);
            c.text(&s, PAD / 2.0 + 30.0, y + 6.0 + t.height, pal.text_dim);
        });
        y += 46.0;
    }
    Ok(())
}

/// Draws the page; returns the y where it ends.
fn page(ui: &Ui, v: &View) -> Result<f32> {
    let mut y = PAD - 6.0;
    if v.back {
        let g = ui.g.glyph(BACK, 11.0)?;
        let t = ui.text(t("Back"), 12.0, false, None)?;
        let w = 10.0 + g.width + 6.0 + t.width + 10.0;
        let hovered = ui.hit(X0 - 10.0, y - 4.0, w, 26.0, Action::Back);
        if let Some(c) = ui.c {
            if hovered {
                c.fill_round_rect(X0 - 10.0, y - 4.0, w, 26.0, 5.0, ui.pal.row_hover);
            }
            c.text(&g, X0, y + (t.height - g.height) / 2.0 + 1.0, ui.pal.accent);
            c.text(&t, X0 + g.width + 6.0, y, ui.pal.accent);
        }
        y += 28.0;
    }
    let message = |text: &str| -> Result<f32> {
        let t = ui.text(text, 14.0, false, Some(W))?;
        if let Some(c) = ui.c {
            c.text(&t, X0, y + 6.0, ui.pal.text_dim);
        }
        Ok(y + 30.0)
    };
    match (v.page, v.data) {
        // How far it is, once that's known (nothing is counted for the storage page).
        (_, Data::Loading) if v.progress.1 > 0 => {
            let (done, total) = (v.progress.0.min(v.progress.1), v.progress.1);
            let y = message(&f("Reading transcripts… {0} of {1}", &[&done, &total]))?;
            if let Some(c) = ui.c {
                let w = 260.0;
                c.fill_round_rect(X0, y, w, 6.0, 3.0, ui.pal.track);
                c.fill_round_rect(X0, y, (w * done as f32 / total as f32).max(6.0), 6.0, 3.0, ui.pal.accent);
            }
            Ok(y + 20.0)
        }
        (
            Page::Overview(Period::All) | Page::Sessions { .. } | Page::Projects | Page::Storage | Page::Weeks,
            Data::Loading,
        ) => message(t("Reading all transcripts… the first time takes a few seconds.")),
        (_, Data::Loading) => message(t("Reading transcripts…")),
        (_, Data::Missing) => message(t("No transcript found for this session.")),
        (Page::Overview(p), Data::Overview(o)) => overview_page(ui, v, (*p, None), o, y),
        (Page::Project { name, period }, Data::Overview(o)) => overview_page(ui, v, (*period, Some(name)), o, y),
        (Page::Projects, Data::Projects(l)) => projects_page(ui, v, l, y),
        (Page::Session(_), Data::Session(d)) => session_page(ui, v, d, y),
        (Page::Calls { session, period, filter }, Data::Calls(l)) => {
            calls_page(ui, v, l, (session.as_ref(), *period), filter, y)
        }
        (Page::Files { .. }, Data::Files(l)) => files_page(ui, v, l, y),
        (Page::Sessions { archived }, Data::Sessions(l)) => sessions_page(ui, v, l, *archived, y),
        (Page::Storage, Data::Storage(s)) => storage_page(ui, s, y),
        (Page::Weeks, Data::Weeks(l)) => weeks_page(ui, v, l, y),
        _ => Ok(y),
    }
}

/// Title and subtitle; returns the y below them.
fn header(ui: &Ui, y: f32, w: f32, title: &str, subtitle: &str) -> Result<f32> {
    let t = ui.text(title, 22.0, true, Some(w))?;
    let s = ui.text(subtitle, 13.0, false, Some(W))?;
    if let Some(c) = ui.c {
        c.text(&t, X0, y, ui.pal.text);
        c.text(&s, X0, y + 2.0 + t.height, ui.pal.text_dim);
    }
    Ok(y + 2.0 + t.height + s.height + 16.0)
}

/// A number on the pages' tile rows.
struct Tile {
    value: String,
    label: String,
    accent: Option<Color>,
    action: Option<Action>,
    tip: Option<String>,
    /// Change against the period before, in percent, and what that period is.
    change: Option<(f64, String)>,
}

/// A tile; `label` is translated.
fn tile(value: String, label: &'static str) -> Tile {
    Tile { value, label: t(label).to_string(), accent: None, action: None, tip: None, change: None }
}

/// The change from `before` to `now` in percent, if there was something before.
fn change(now: f64, before: f64) -> Option<f64> {
    (before > 0.0).then(|| (now - before) / before * 100.0)
}

impl Tile {
    fn accent(self, accent: Option<Color>) -> Tile {
        Tile { accent, ..self }
    }

    fn go(self, action: Option<Action>) -> Tile {
        Tile { action, ..self }
    }

    fn tip(self, tip: String) -> Tile {
        Tile { tip: Some(tip), ..self }
    }

    /// Compares with the period before, when there is one.
    fn versus(self, compare: Option<(f64, f64, &str)>) -> Tile {
        let change = compare.and_then(|(now, before, what)| change(now, before).map(|c| (c, what.to_string())));
        Tile { change, ..self }
    }
}

fn tiles(ui: &Ui, y: f32, items: &[Tile]) -> Result<f32> {
    let n = items.len() as f32;
    let tw = (W - GAP * (n - 1.0)) / n;
    // Taller when a tile compares with the period before, which takes a line.
    let h = if items.iter().any(|t| t.change.is_some()) { 92.0 } else { 76.0 };
    for (i, t) in items.iter().enumerate() {
        ui.tile(X0 + i as f32 * (tw + GAP), y, tw, h, t)?;
    }
    Ok(y + h + GAP)
}

/// The two rows of numbers of a period or a session: `first` and `last`
/// complete the second row.
#[allow(clippy::too_many_arguments, reason = "the numbers, the period before, links and extra tiles")]
fn summary_tiles(
    ui: &Ui,
    y: f32,
    s: &stats::Summary,
    before: Option<(&stats::Summary, &str)>,
    calls: &dyn Fn(Filter) -> Option<Action>,
    files: Option<Action>,
    (first, last): (Option<Tile>, Option<Tile>),
) -> Result<f32> {
    let pal = ui.pal;
    // (now, before, what "before" is) for a number of the summary.
    let vs = |f: &dyn Fn(&stats::Summary) -> f64| before.map(|(b, what)| (f(s), f(b), what));
    let context = s.tokens.context();
    let y = tiles(
        ui,
        y,
        &[
            tile(num(s.prompts.into()), "prompts").accent(Some(pal.accent)).versus(vs(&|x| f64::from(x.prompts))),
            tile(num(s.replies.into()), "replies").versus(vs(&|x| f64::from(x.replies))),
            tile(num(s.tool_calls.into()), "tool calls")
                .go(calls(Filter::All))
                .versus(vs(&|x| f64::from(x.tool_calls))),
            tile(num(s.agents.into()), "agents started").tip(t("Subagents Claude started with the Agent tool.").into()),
            tile(tokens(s.output()), "tokens written")
                .accent(Some(pal.done))
                .tip(f(
                    "{0} by the session itself, {1} by subagents. {2} of it was thinking.",
                    &[&tokens(s.tokens.output), &tokens(s.agent_output), &tokens(s.tokens.thinking)],
                ))
                .versus(vs(&|x| x.output() as f64)),
            tile(percent(s.tokens.cache_hit()), "from cache").tip(f(
                "{0} of {1} input tokens were read from the prompt cache, which is cheaper and faster.",
                &[&tokens(s.tokens.cache_read), &tokens(context)],
            )),
        ],
    )?;
    let mut row: Vec<Tile> = first.into_iter().collect();
    row.extend([
        tile(num(s.errors.into()), "failed calls").accent((s.errors > 0).then_some(pal.crit)).go(calls(Filter::Failed)),
        tile(num(s.files_changed.into()), "files changed").go(files),
        tile(format!("+{}", num(s.added)), "lines written")
            .tip(f("{0} lines written and {1} lines replaced by edits.", &[&s.added, &s.removed])),
        tile(num(s.commits.into()), "commits").tip(t("Successful git commit commands.").into()),
        tile(num(s.web.into()), "web lookups").tip(t("Web searches and page fetches.").into()),
    ]);
    row.extend(last);
    let y = tiles(ui, y, &row)?;
    let best = |v: Option<i64>| v.map_or("–".to_string(), turn_time);
    tiles(
        ui,
        y,
        &[
            tile(stats::dollars(s.total_cost()), "API value").accent(Some(pal.plan_to)).tip(f(
                "What these tokens would cost at Anthropic's API prices ({0} of it by subagents). Your plan is billed differently.",
                &[&stats::dollars(s.agent_cost)],
            ))
            .versus(vs(&|x| x.total_cost())),
            tile(num(s.turns.len() as u64), "turns")
                .tip(t("Prompts that were worked on.").into())
                .versus(vs(&|x| x.turns.len() as f64)),
            tile(duration(s.working_secs()), "time working")
                .tip(t("From each prompt to its last step, added up. Includes time spent waiting for your answers.").into())
                .versus(vs(&|x| x.working_secs() as f64)),
            tile(best(s.median_turn()), "median turn").tip(t("Half of the turns were shorter than this.").into()),
            tile(best(s.longest_turn()), "longest turn"),
        ],
    )
}

/// Widths of a two-column row.
fn columns() -> (f32, f32, f32) {
    let left_w = (W - GAP) * 0.58;
    (left_w, X0 + left_w + GAP, W - GAP - left_w)
}

fn list_card_h(rows: usize) -> f32 {
    12.0 + 18.0 + 10.0 + rows.max(1) as f32 * 22.0 + 8.0
}

/// The overview of all sessions in a period, or of one project's (`project`):
/// a project's page has its models in detail where the weekly limit is (the
/// limit isn't a project's), and its sessions where the projects are; its
/// numbers don't link to lists of all sessions' calls and files.
fn overview_page(ui: &Ui, v: &View, (period, project): (Period, Option<&String>), o: &Overview, y: f32) -> Result<f32> {
    let s = &o.summary;
    let swap = |p: Period| match project {
        Some(name) => Page::Project { name: name.clone(), period: p },
        None => Page::Overview(p),
    };
    let periods: Vec<(&str, bool, Action)> =
        Period::ALL.iter().map(|&p| (t(p.label()), p == period, Action::Swap(swap(p)))).collect();
    ui.segmented(X0 + W, y + 2.0, &periods)?;
    ui.button(X0 + W - 330.0, y + 2.0, DOWNLOAD, "CSV", Action::Export)?;
    let since = match period {
        Period::Today => t("Today").to_string(),
        Period::All => o
            .buckets
            .first()
            .map_or(t("All time").into(), |(at, _)| f("Since {0}", &[&timefmt::local_date(*at + 12 * HOUR)])),
        p => f("Last {0}", &[&t(p.label())]),
    };
    let sessions_text = plural(o.sessions.into(), "{0} session", "{0} sessions");
    let hours = plural(o.active_hours.into(), "{0} active hour", "{0} active hours");
    let subtitle = match project {
        Some(_) => format!("{since} · {sessions_text} · {hours}"),
        None => format!(
            "{since} · {sessions_text} {} · {hours}",
            plural(o.projects.len() as u64, "in {0} project", "in {0} projects")
        ),
    };
    let title = project.map_or(t("Overview"), String::as_str);
    let mut y = header(ui, y, W - 420.0, title, &subtitle)?;
    let all = project.is_none();
    let calls = |filter: Filter| all.then_some(Action::Go(Page::Calls { session: None, period, filter }));
    let sessions =
        tile(num(o.sessions.into()), "sessions").go(all.then_some(Action::Go(Page::Sessions { archived: None })));
    let before = match period {
        Period::Today => t("yesterday until this time"),
        Period::Week => t("the 7 days before"),
        Period::Month => t("the 30 days before"),
        Period::All => "",
    };
    y = summary_tiles(
        ui,
        y,
        s,
        o.previous.as_ref().map(|p| (p, before)),
        &calls,
        all.then_some(Action::Go(Page::Files { session: None, period })),
        (Some(sessions), None),
    )?;

    let (left_w, right_x, right_w) = columns();
    // Tokens per bucket.
    let chart_title = match o.bucket {
        HOUR => t("Tokens written per hour"),
        DAY => t("Tokens written per day"),
        _ => t("Tokens written per week"),
    };
    let cy = ui.card(X0, y, left_w, CHART_H, chart_title, t("incl. agents"), None)?;
    let n = o.buckets.len();
    let bars: Vec<Bar> = o
        .buckets
        .iter()
        .enumerate()
        .map(|(i, (t, a))| Bar {
            label: bucket_label(*t, o.bucket, i, n),
            value: (a.output + a.agent_output) as f64,
            title: bucket_title(*t, o.bucket),
            lines: activity_lines(a),
        })
        .collect();
    let current = o.buckets.iter().rposition(|(t, _)| *t <= v.m.now);
    ui.bars(X0 + 16.0, cy, left_w - 32.0, y + CHART_H - cy - 12.0, &bars, current)?;
    if project.is_some() {
        // The project's sessions, by what they wrote.
        let rows: Vec<(String, f64)> = o.top.iter().map(|s| (s.title.clone(), s.output as f64)).collect();
        let go = |title: &str| o.top.iter().find(|s| s.title == title).map(|s| Action::Go(Page::Session(s.id.clone())));
        let cy = ui.card(right_x, y, right_w, CHART_H, t("Sessions"), t("tokens written"), None)?;
        ui.ranked(right_x + 16.0, cy, right_w - 32.0, &rows, 8, |v| tokens(v as u64), go)?;
        y += CHART_H + GAP;
        models_card(ui, X0, y, left_w, CHART_H, &o.models)?;
    } else {
        // Each project leads to its own page.
        let projects: Vec<(String, f64)> = o.projects.iter().map(|p| (p.name.clone(), p.output as f64)).collect();
        let cy = ui.card(right_x, y, right_w, CHART_H, t("Projects"), t("tokens written"), None)?;
        let go = |name: &str| Some(Action::Go(Page::Project { name: name.to_string(), period }));
        ui.ranked(right_x + 16.0, cy, right_w - 32.0, &projects, 8, |v| tokens(v as u64), go)?;
        y += CHART_H + GAP;
        // The weekly limit over its window, against an even pace.
        weekly_chart(ui, v.m, X0, y, left_w)?;
    }
    let tools: Vec<(String, f64)> = o.tools.iter().map(|(k, v)| (k.clone(), f64::from(*v))).collect();
    let cy = ui.card(
        right_x,
        y,
        right_w,
        CHART_H,
        t("Tools"),
        "",
        all.then(|| (t("All calls"), Action::Go(Page::Calls { session: None, period, filter: Filter::All }))),
    )?;
    ui.ranked(
        right_x + 16.0,
        cy,
        right_w - 32.0,
        &tools,
        8,
        |v| format!("{v:.0}"),
        |name| all.then(|| Action::Go(Page::Calls { session: None, period, filter: Filter::Tool(name.to_string()) })),
    )?;
    y += CHART_H + GAP;

    // What sessions start with: the context before the first reply, and what's loaded.
    let mut sizes: Vec<u64> = o.starts.iter().map(|s| s.1).collect();
    sizes.sort_unstable();
    let note = sizes.get(sizes.len() / 2).map_or(String::new(), |m| f("median {0}", &[&tokens(*m)]));
    let cy = ui.card(X0, y, left_w, CHART_H, t("Starting context"), &note, None)?;
    let points: Vec<(i64, f64)> = o.starts.iter().map(|&(t, v)| (t, v as f64)).collect();
    let span = (o.starts.first().map_or(o.from, |s| s.0), o.starts.last().map_or(o.to, |s| s.0));
    let max = sizes.last().copied().unwrap_or(0) as f64 * 1.15;
    let ch = y + CHART_H - cy - 26.0;
    let now = v.m.now;
    if o.starts.is_empty() {
        if let Some(c) = ui.c {
            let n = ui.text(t("Not in the transcripts of this period."), 12.0, false, None)?;
            c.text(&n, X0 + 16.0, cy, ui.pal.text_dim);
        }
    } else {
        ui.area_chart(X0 + 16.0, cy, left_w - 32.0, ch, &points, span, max, ui.pal.plan_to, &|t, v| {
            (when(t, now), vec![f("{0} tokens before the first reply", &[&tokens(v as u64)])])
        });
        if let Some(c) = ui.c {
            let a = ui.text(&when(span.0, now), 11.0, false, None)?;
            let b = ui.text(&when(span.1, now), 11.0, false, None)?;
            c.text(&a, X0 + 16.0, cy + ch + 6.0, ui.pal.text_dim);
            c.text(&b, X0 + left_w - 16.0 - b.width, cy + ch + 6.0, ui.pal.text_dim);
        }
    }
    let mut rows = o.setup.as_ref().map(|(s, _)| setup_rows(s)).unwrap_or_default();
    rows.push((t("Memory files"), f("{0} in all projects", &[&o.memory_files])));
    setup_card(ui, right_x, y, right_w, t("as of the latest session"), &rows)?;
    y += CHART_H + GAP;

    // When the work happens.
    let heat_h = 12.0 + 18.0 + 10.0 + 7.0 * 21.0 + 18.0 + 14.0;
    let cy =
        ui.card(X0, y, W, heat_h, t("When you work"), t("prompts and replies by weekday and hour, local time"), None)?;
    ui.heatmap(X0 + 16.0, cy, W - 32.0, &o.heat)?;
    y += heat_h + GAP;

    let turns: Vec<(&Turn, Option<(&str, &str)>)> =
        o.longest.iter().map(|t| (&t.turn, Some((t.id.as_str(), t.title.as_str())))).collect();
    y = turns_card(ui, v, y, &turns)?;
    let page = |filter: Filter| Page::Calls { session: None, period, filter };
    y = failures_card(ui, v, y, &o.failures, &page)?;

    // Commands and models.
    let half = (W - GAP) / 2.0;
    let commands: Vec<(String, f64)> = o.commands.iter().map(|(k, v)| (k.clone(), f64::from(*v))).collect();
    // A project's page has its models above, in detail: the commands take the whole width.
    let total: u64 = o.models.iter().map(|m| m.output).sum::<u64>().max(1);
    let models: Vec<(String, f64)> = o.models.iter().map(|m| (m.name.clone(), m.output as f64)).collect();
    let h = list_card_h(commands.len().max(if all { models.len() } else { 0 }).min(8));
    let commands_w = if all { half } else { W };
    let cy = ui.card(X0, y, commands_w, h, t("Commands"), t("programs run in the shell"), None)?;
    ui.ranked(X0 + 16.0, cy, commands_w - 32.0, &commands, 8, |v| format!("{v:.0}"), |_| None)?;
    if all {
        let cy = ui.card(X0 + half + GAP, y, half, h, t("Models"), t("share of tokens written"), None)?;
        let share = |v: f64| format!("{:.0}% · {}", v / total as f64 * 100.0, tokens(v as u64));
        ui.ranked(X0 + half + GAP + 16.0, cy, half - 32.0, &models, 8, share, |_| None)?;
    }
    y += h + GAP;

    // The sessions that wrote the most.
    let rows_h = o.top.len().max(1) as f32 * 50.0;
    let h = 12.0 + 18.0 + 10.0 + rows_h + 8.0;
    let cy = ui.card(
        X0,
        y,
        W,
        h,
        t("Top sessions"),
        t("by tokens written"),
        Some((t("All sessions"), Action::Go(Page::Sessions { archived: None }))),
    )?;
    let mut ry = cy - 4.0;
    for s in &o.top {
        session_row(ui, v.m.now, X0 + 6.0, ry, W - 12.0, s)?;
        ry += 50.0;
    }
    Ok(y + h)
}

/// The weekly limit chart.
fn weekly_chart(ui: &Ui, m: &Model, x: f32, y: f32, w: f32) -> Result<()> {
    let pal = ui.pal;
    // Earlier weekly windows from the history, newest first: (reset, samples by time into the week).
    let mut earlier: Vec<(i64, Vec<(i64, f64)>)> = Vec::new();
    if let Some(reset) = m.weekly_reset {
        for s in &m.history {
            let Some(r) = s.resets_at.filter(|r| *r < reset - 3600) else { continue };
            if s.at < r - forecast::WEEK || s.at > r {
                continue;
            }
            match earlier.iter_mut().find(|(r0, _)| (r0 - r).abs() < 3600) {
                Some((_, pts)) => pts.push((s.at - (r - forecast::WEEK), s.used)),
                None => earlier.push((r, vec![(s.at - (r - forecast::WEEK), s.used)])),
            }
        }
    }
    earlier.sort_by_key(|(r, _)| std::cmp::Reverse(*r));
    earlier.truncate(3);
    let ago = |i: usize| if i == 0 { t("last week").to_string() } else { f("{0} weeks ago", &[&(i + 1)]) };
    let mut note = m.history.last().map_or(String::new(), |s| f("{0}% now", &[&format!("{:.0}", s.used)]));
    if let Some((_, pts)) = earlier.first() {
        if let Some((_, used)) = pts.iter().max_by_key(|(off, _)| *off) {
            note += &format!(" · {}", f("last week ended at {0}%", &[&format!("{used:.0}")]));
        }
    }
    let cy = ui.card(x, y, w, CHART_H, t("Weekly limit"), &note, None)?;
    let Some(reset) = m.weekly_reset else {
        if let Some(c) = ui.c {
            let t = ui.text(t("No weekly limit data yet."), 12.0, false, None)?;
            c.text(&t, x + 16.0, cy, pal.text_dim);
        }
        return Ok(());
    };
    let start = reset - forecast::WEEK;
    let (cx, cw, ch) = (x + 16.0, w - 32.0, y + CHART_H - cy - 26.0);
    let samples: Vec<(i64, f64)> = m
        .history
        .iter()
        .filter(|s| s.at >= start && s.resets_at.is_some_and(|r| (r - reset).abs() < 3600))
        .map(|s| (s.at, s.used))
        .collect();
    if let Some(c) = ui.c {
        // The whole limit over the whole week: even pace is the diagonal.
        c.polyline(&[(cx, cy + ch), (cx + cw, cy)], 1.2, pal.text_dim.with_alpha(0.5));
        // Earlier weeks, fainter the older they are.
        for (i, (_, pts)) in earlier.iter().enumerate() {
            let line: Vec<(f32, f32)> = pts
                .iter()
                .map(|&(off, used)| {
                    (
                        cx + cw * (off as f32 / forecast::WEEK as f32),
                        cy + ch - ch * (used / 100.0).clamp(0.0, 1.0) as f32,
                    )
                })
                .collect();
            if line.len() >= 2 {
                c.polyline(&line, 1.4, pal.text_dim.with_alpha(0.75 - 0.2 * i as f32));
            }
        }
    }
    let even = |t: i64| (t - start) as f64 / forecast::WEEK as f64 * 100.0;
    ui.area_chart(cx, cy, cw, ch, &samples, (start, reset), 100.0, pal.accent, &|t, used| {
        let mut lines = vec![
            i18n::f("{0}% used", &[&format!("{used:.0}")]),
            i18n::f("{0}% at an even pace", &[&format!("{:.0}", even(t))]),
        ];
        let off = t - start;
        for (i, (_, pts)) in earlier.iter().enumerate() {
            if let Some((o, u)) = pts.iter().min_by_key(|(o, _)| (o - off).abs()) {
                if (o - off).abs() <= 6 * 3600 {
                    lines.push(format!("{}: {u:.0}%", ago(i)));
                }
            }
        }
        (timefmt::local_time(t, true), lines)
    });
    if let Some(c) = ui.c {
        if ui.mouse.is_none_or(|(mx, my)| !(mx >= cx && mx < cx + cw && my >= cy && my < cy + ch)) {
            let now_x = cx + cw * ((m.now - start) as f32 / forecast::WEEK as f32).clamp(0.0, 1.0);
            if let Some(&(_, used)) = samples.last() {
                c.fill_circle(now_x, cy + ch - (used / 100.0) as f32 * ch, 4.0, pal.accent);
            }
        }
        let legend = if earlier.is_empty() {
            t("diagonal: even pace over the week")
        } else {
            t("diagonal: even pace · grey: earlier weeks")
        };
        let legend = ui.text(legend, 11.0, false, None)?;
        c.text(&legend, cx, cy + ch + 6.0, pal.text_dim);
        let end = ui.text(&f("resets {0}", &[&timefmt::local_time(reset, true)]), 11.0, false, None)?;
        c.text(&end, cx + cw - end.width, cy + ch + 6.0, pal.text_dim);
    }
    Ok(())
}

/// A session in a list, clickable.
/// The models of a project in a table, a chart card high: each one's share of
/// the tokens written, the tokens it wrote and read, its replies and API value.
fn models_card(ui: &Ui, x: f32, y: f32, w: f32, h: f32, models: &[stats::ModelUse]) -> Result<()> {
    let cy = ui.card(x, y, w, h, t("Models"), t("incl. agents · at API prices"), None)?;
    let Some(c) = ui.c else { return Ok(()) };
    let pal = ui.pal;
    if models.is_empty() {
        let n = ui.text(t("Not in the transcripts of this period."), 12.0, false, None)?;
        c.text(&n, x + 16.0, cy, pal.text_dim);
        return Ok(());
    }
    let total: u64 = models.iter().map(|m| m.output).sum::<u64>().max(1);
    let heads = [t("share"), t("written"), t("read"), t("replies"), t("API value")];
    let col = 62.0;
    let right = |i: usize| x + w - 16.0 - (heads.len() - 1 - i) as f32 * col;
    for (i, head) in heads.iter().enumerate() {
        let h = ui.text(head, 11.0, false, None)?;
        c.text(&h, right(i) - h.width, cy, pal.text_dim);
    }
    let mut ry = cy + 22.0;
    for m in models.iter().take(7) {
        let name_w = right(0) - col - (x + 16.0);
        let name = ui.text(&m.name, 12.5, true, Some(name_w))?;
        c.text(&name, x + 16.0, ry, pal.text);
        let share = m.output as f64 / total as f64;
        let values = [
            format!("{:.0}%", share * 100.0),
            tokens(m.output),
            tokens(m.input),
            num(m.replies.into()),
            stats::dollars(m.cost),
        ];
        for (i, v) in values.iter().enumerate() {
            let t = ui.text(v, 12.5, i == 0, None)?;
            c.text(&t, right(i) - t.width, ry, if i == 0 { pal.accent } else { pal.text });
        }
        // How much of the writing it did, as a thin bar under its name.
        let bar_w = name_w.min(160.0);
        c.fill_round_rect(x + 16.0, ry + name.height + 2.0, bar_w, 3.0, 1.5, pal.track);
        c.fill_round_rect(x + 16.0, ry + name.height + 2.0, (bar_w * share as f32).max(3.0), 3.0, 1.5, pal.accent);
        ry += name.height + 12.0;
    }
    Ok(())
}

/// Every week, the latest first: what the weekly summary says of it. This week
/// is counted as far as it is; weeks that are over show as they were saved.
fn weeks_page(ui: &Ui, v: &View, l: &[Week], y: f32) -> Result<f32> {
    let subtitle = f(
        "{0} · {1}",
        &[
            &plural(l.len() as u64, "{0} week", "{0} weeks"),
            &t("Monday to Sunday; a week's numbers are saved once it's over"),
        ],
    );
    ui.button(X0 + W, y + 2.0, DOWNLOAD, "CSV", Action::Export)?;
    let mut y = header(ui, y, W - 150.0, t("Weeks"), &subtitle)?;
    let offsets = timefmt::offsets();
    let this_week = crate::alerts::week_of(v.m.now, &offsets);
    let most = l.iter().map(|w| w.tokens).max().unwrap_or(0).max(1);
    for w in l {
        week_row(ui, X0, y, W, w, (this_week, most), &offsets)?;
        y += 50.0;
    }
    Ok(y)
}

/// The days of the week that starts at `start`, e.g. `9 Nov – 15 Nov`, with the
/// year when it isn't this one (`this_week` starts the current week).
fn week_days(start: i64, this_week: i64, offsets: &dyn Fn(i64) -> i64) -> String {
    let end = crate::alerts::next_week(start, offsets);
    let days = format!("{} – {}", timefmt::local_date(start), timefmt::local_date(end - 1));
    // Local time's year, read off the date as text.
    let year = |t: i64| timefmt::rfc3339(t + offsets(t)).chars().take(4).collect::<String>();
    if year(end - 1) == year(this_week) { days } else { format!("{days} {}", year(end - 1)) }
}

/// A week in the list; `at` is the current week's start and the most tokens any week has.
fn week_row(ui: &Ui, x: f32, y: f32, w: f32, week: &Week, at: (i64, u64), offsets: &dyn Fn(i64) -> i64) -> Result<()> {
    let h = 48.0;
    if !ui.shown(y, h) {
        return Ok(());
    }
    let Some(c) = ui.c else { return Ok(()) };
    let pal = ui.pal;
    let (this_week, most) = at;
    let cols = [
        (t("prompts"), num(week.prompts.into())),
        (t("tokens"), tokens(week.tokens)),
        (t("API value"), stats::dollars(week.cost)),
        (t("Claude worked"), duration(week.working)),
    ];
    let col_w = 86.0;
    let bar_w = 130.0;
    let text_w = w - 20.0 - cols.len() as f32 * col_w - bar_w - 36.0;
    let title = ui.text(&week_days(week.start, this_week, offsets), 13.0, true, Some(text_w))?;
    c.text(&title, x + 10.0, y + 7.0, pal.text);
    let mut sub = Vec::new();
    if week.start == this_week {
        sub.push(t("This week").to_string());
    } else if crate::alerts::next_week(week.start, offsets) == this_week {
        sub.push(t("Last week").to_string());
    }
    if week.sessions > 0 {
        sub.push(plural(week.sessions.into(), "{0} session", "{0} sessions"));
    }
    let sub = ui.text(&sub.join(" · "), 12.0, false, Some(text_w))?;
    c.text(&sub, x + 10.0, y + 9.0 + title.height, pal.text_dim);
    // How much was written, against the week with the most.
    let bar_x = x + 10.0 + text_w + 12.0;
    let share = week.tokens as f32 / most as f32;
    c.fill_round_rect(bar_x, y + h / 2.0 - 3.0, bar_w, 6.0, 3.0, pal.track);
    if week.tokens > 0 {
        c.fill_round_rect(bar_x, y + h / 2.0 - 3.0, (bar_w * share).max(6.0), 6.0, 3.0, pal.accent);
    }
    for (i, (label, value)) in cols.iter().enumerate() {
        let right = x + w - 10.0 - (cols.len() - 1 - i) as f32 * col_w;
        let vt = ui.text(value, 13.0, true, None)?;
        let lt = ui.text(label, 11.0, false, None)?;
        c.text(&vt, right - vt.width, y + 7.0, pal.text);
        c.text(&lt, right - lt.width, y + 9.0 + vt.height, pal.text_dim);
    }
    Ok(())
}

/// Every project: its sessions summed, the latest used first. A project leads to its page.
fn projects_page(ui: &Ui, v: &View, l: &[stats::ProjectInfo], y: f32) -> Result<f32> {
    let subtitle = f(
        "{0} · {1}",
        &[
            &plural(l.len() as u64, "{0} project", "{0} projects"),
            &plural(l.iter().map(|p| u64::from(p.sessions)).sum(), "{0} session", "{0} sessions"),
        ],
    );
    ui.button(X0 + W, y + 2.0, DOWNLOAD, "CSV", Action::Export)?;
    let mut y = header(ui, y, W - 150.0, t("Projects"), &subtitle)?;
    let o = v.order;
    let sorts = sort_items(&ProjectSort::ALL, o.projects, o.projects_desc, ProjectSort::label, Action::SortProjects);
    y = ui.sort_row(y, t("Sort by"), &sorts)?;
    let picked = memo(&("projects", format!("{:?}", v.order)), || indices(l, &project_rows(l, &v.order)));
    for p in picked.iter().filter_map(|i| l.get(*i)) {
        project_row(ui, v.m.now, X0, y, W, p)?;
        y += 50.0;
    }
    Ok(y)
}

/// A project in the list; it opens on all time, as the list counts.
fn project_row(ui: &Ui, now: i64, x: f32, y: f32, w: f32, p: &stats::ProjectInfo) -> Result<()> {
    let h = 48.0;
    let hovered = ui.hit(x, y, w, h, Action::Go(Page::Project { name: p.name.clone(), period: Period::All }));
    if !ui.shown(y, h) {
        return Ok(());
    }
    let Some(c) = ui.c else { return Ok(()) };
    let pal = ui.pal;
    if hovered {
        c.fill_round_rect(x, y, w, h, 6.0, pal.row_hover);
    }
    let cols = [
        (t("sessions"), num(p.sessions.into())),
        (t("prompts"), num(p.prompts.into())),
        (t("tokens"), tokens(p.output)),
        (t("API value"), stats::dollars(p.cost)),
    ];
    let col_w = 76.0;
    let text_w = w - 20.0 - cols.len() as f32 * col_w - 12.0;
    let title = ui.text(&p.name, 13.0, true, Some(text_w))?;
    c.text(&title, x + 10.0, y + 7.0, if hovered { pal.accent } else { pal.text });
    let mut sub = Vec::new();
    if let Some(last) = p.last_at {
        sub.push(f("last used {0}", &[&when(last, now)]));
    }
    if let Some(first) = p.first_at {
        sub.push(f("since {0}", &[&timefmt::local_date(first)]));
    }
    if p.errors > 0 {
        sub.push(plural(p.errors.into(), "{0} failed call", "{0} failed calls"));
    }
    let sub = ui.text(&sub.join(" · "), 12.0, false, Some(text_w))?;
    c.text(&sub, x + 10.0, y + 9.0 + title.height, pal.text_dim);
    for (i, (label, value)) in cols.iter().enumerate() {
        let right = x + w - 10.0 - (cols.len() - 1 - i) as f32 * col_w;
        let vt = ui.text(value, 13.0, true, None)?;
        let lt = ui.text(label, 11.0, false, None)?;
        c.text(&vt, right - vt.width, y + 7.0, pal.text);
        c.text(&lt, right - lt.width, y + 9.0 + vt.height, pal.text_dim);
    }
    Ok(())
}

fn session_row(ui: &Ui, now: i64, x: f32, y: f32, w: f32, s: &SessionInfo) -> Result<()> {
    let h = 48.0;
    let hovered = ui.hit(x, y, w, h, Action::Go(Page::Session(s.id.clone())));
    if !ui.shown(y, h) {
        return Ok(());
    }
    let Some(c) = ui.c else { return Ok(()) };
    let pal = ui.pal;
    if hovered {
        c.fill_round_rect(x, y, w, h, 6.0, pal.row_hover);
    }
    let cols = [
        (t("prompts"), num(s.prompts.into())),
        (t("tokens"), tokens(s.output)),
        (t("API value"), stats::dollars(s.cost)),
        (t("size"), stats::bytes(s.size.bytes)),
    ];
    let show_size = s.size.bytes > 0;
    let col_w = 76.0;
    let ncols = if show_size { 4.0 } else { 3.0 };
    let text_w = w - 20.0 - ncols * col_w - 12.0;
    let mut tag_w = 0.0;
    if s.archived {
        let tag = ui.text(t("archived"), 10.5, false, None)?;
        tag_w = tag.width + 14.0;
        let tx = x + 10.0 + text_w - tag_w;
        c.fill_round_rect(tx, y + 8.0, tag.width + 10.0, tag.height + 2.0, 4.0, pal.track.with_alpha(0.6));
        c.text(&tag, tx + 5.0, y + 9.0, pal.text_dim);
    }
    let title = ui.text(&s.title, 13.0, true, Some(text_w - tag_w))?;
    c.text(&title, x + 10.0, y + 7.0, if hovered { pal.accent } else { pal.text });
    let mut sub = vec![s.project.clone()];
    if let Some(d) = &s.distro {
        sub.push(format!("WSL {d}"));
    }
    if let (Some(a), Some(b)) = (s.first_at, s.last_at) {
        sub.push(format!("{} · {}", when(b, now), duration(b - a)));
    }
    if s.errors > 0 {
        sub.push(plural(s.errors.into(), "{0} failed call", "{0} failed calls"));
    }
    let sub = ui.text(&sub.join(" · "), 12.0, false, Some(text_w))?;
    c.text(&sub, x + 10.0, y + 9.0 + title.height, pal.text_dim);
    for (i, (label, value)) in cols.iter().take(ncols as usize).enumerate() {
        let right = x + w - 10.0 - (ncols as usize - 1 - i) as f32 * col_w;
        let vt = ui.text(value, 13.0, true, None)?;
        let lt = ui.text(label, 11.0, false, None)?;
        c.text(&vt, right - vt.width, y + 7.0, pal.text);
        c.text(&lt, right - lt.width, y + 9.0 + vt.height, pal.text_dim);
    }
    Ok(())
}

fn session_page(ui: &Ui, v: &View, d: &SessionData, y: f32) -> Result<f32> {
    let pal = ui.pal;
    let s = &d.stats;
    let sum = &d.summary;
    let entry = v.m.sessions.iter().find(|e| e.id == d.id);
    let title = s.title.clone().or(entry.map(|e| e.title.clone())).unwrap_or_else(|| s.display_title());
    let mut sub = Vec::new();
    match (entry, &s.cwd) {
        (Some(e), _) if !e.workspace.is_empty() => sub.push(e.workspace.clone()),
        (_, Some(cwd)) => sub.push(stats::project_name(cwd)),
        _ => {}
    }
    if let Some(distro) = &d.distro {
        sub.push(format!("WSL {distro}"));
    }
    if let Some(m) = s.main_model() {
        sub.push(m);
    }
    if let (Some(first), Some(last)) = (s.first_at, s.last_at) {
        sub.push(f("started {0}", &[&when(first, v.m.now)]));
        sub.push(f("active over {0}", &[&duration(last - first)]));
    }
    // Folder buttons right of the title, and the command that continues the session.
    let mut right = X0 + W;
    // Kept statistics: the transcript is gone, so it can't be resumed.
    if d.kept {
        sub.push(t("transcript deleted, statistics kept").to_string());
    }
    if let Some(cwd) = s.cwd.as_ref().filter(|_| !d.kept) {
        // A WSL session's command is for its shell there, a Windows one's for PowerShell.
        let word = |text: &str| shell_word(text, d.distro.is_some());
        let resume = match d.id.strip_prefix(crate::copilot::ID_PREFIX) {
            Some(id) => format!("cd {}; copilot --resume {}", word(cwd), word(id)),
            None => format!("cd {}; claude --resume {}", word(cwd), word(&d.id)),
        };
        let done = v.copied == Some(resume.as_str());
        let (glyph, label) = if done { (CHECK, t("Copied")) } else { (COPY, t("Resume command")) };
        right -= ui.button(right, y + 2.0, glyph, label, Action::Copy(resume))? + 8.0;
    }
    if let Some(dir) = d.transcript.parent().filter(|_| !d.kept) {
        right -= ui.button(right, y + 2.0, FOLDER, t("Transcripts"), Action::Open(dir.to_path_buf()))? + 8.0;
    }
    if let Some(cwd) = &s.cwd {
        let project = Action::Open(local_path(cwd, d.distro.as_ref()));
        right -= ui.button(right, y + 2.0, FOLDER, t("Project"), project)? + 8.0;
    }
    let mut y = header(ui, y, right - X0 - 12.0, &title, &sub.join(" · "))?;
    let id = Some(d.id.clone());
    let calls = |filter: Filter| Some(Action::Go(Page::Calls { session: id.clone(), period: Period::All, filter }));
    let files = Some(Action::Go(Page::Files { session: id.clone(), period: Period::All }));
    let compactions = tile(num(sum.compactions.into()), "compactions")
        .tip(t("Times the conversation was summarised to make room in the context.").into());
    y = summary_tiles(ui, y, sum, None, &calls, files.clone(), (None, Some(compactions)))?;

    let (left_w, right_x, right_w) = columns();
    // Context size over time; drops are compactions.
    let peak = d.context.iter().map(|c| c.1).max().unwrap_or(0);
    let window = s.replies.last().and_then(|r| s.model(r)).and_then(crate::transcripts::context_window);
    let note = match window {
        Some(w) => f("peak {0} of {1}", &[&tokens(peak), &tokens(w)]),
        None => f("peak {0}", &[&tokens(peak)]),
    };
    let cy = ui.card(X0, y, left_w, CHART_H, t("Context size"), &note, None)?;
    let (first, last) = (s.first_at.unwrap_or(0), s.last_at.unwrap_or(1));
    let points: Vec<(i64, f64)> = d.context.iter().map(|&(t, v)| (t, v as f64)).collect();
    let ch = y + CHART_H - cy - 26.0;
    let now = v.m.now;
    ui.area_chart(X0 + 16.0, cy, left_w - 32.0, ch, &points, (first, last), peak as f64 * 1.1, pal.accent, &|t, v| {
        (when(t, now), vec![f("{0} tokens of context", &[&tokens(v as u64)])])
    });
    if let Some(c) = ui.c {
        let a = ui.text(&when(first, now), 11.0, false, None)?;
        let b = ui.text(&when(last, now), 11.0, false, None)?;
        c.text(&a, X0 + 16.0, cy + ch + 6.0, pal.text_dim);
        c.text(&b, X0 + left_w - 16.0 - b.width, cy + ch + 6.0, pal.text_dim);
    }
    token_mix(ui, right_x, y, right_w, sum)?;
    y += CHART_H + GAP;

    // Which models wrote it, and what its subagents did, summed up.
    let agents = agent_rows(d);
    let h = (68.0 + d.models.len().clamp(1, 7) as f32 * 30.0).max(54.0 + agents.len().max(1) as f32 * 26.0);
    models_card(ui, X0, y, left_w, h, &d.models)?;
    rows_card(ui, (right_x, y, right_w, h), t("Agents"), t("This session started none."), &agents)?;
    y += h + GAP;

    // The subagents it started.
    if !d.agents.is_empty() {
        y = agent_map(ui, v, d, &title, y)? + GAP;
    }

    // What the session started with.
    starting_card(ui, X0, y, left_w, &s.setup, s.starting_context())?;
    let mut rows = if s.setup.known { setup_rows(&s.setup) } else { Vec::new() };
    if let Some(n) = d.memory_files {
        rows.push((t("Memory files"), f("{0} in this project", &[&n])));
    }
    setup_card(ui, right_x, y, right_w, "", &rows)?;
    y += CHART_H + GAP;

    // Worked out once per data, not for every frame.
    let tallies = memo(&("session", &d.id), || session_tallies(s));
    // Activity per hour (active hours only).
    let hours: Vec<(&i64, &Activity)> = tallies.hourly.iter().collect();
    let shown = hours.get(hours.len().saturating_sub(36)..).unwrap_or_default();
    let cy = ui.card(
        X0,
        y,
        left_w,
        CHART_H,
        t("Tokens written per hour"),
        &plural(hours.len() as u64, "{0} active hour", "{0} active hours"),
        None,
    )?;
    let bars: Vec<Bar> = shown
        .iter()
        .enumerate()
        .map(|(i, (t, a))| Bar {
            label: if i % 4 == 0 || shown.len() <= 8 { timefmt::local_time(**t, false) } else { String::new() },
            value: a.output as f64,
            title: bucket_title(**t, HOUR),
            lines: activity_lines(a),
        })
        .collect();
    ui.bars(X0 + 16.0, cy, left_w - 32.0, y + CHART_H - cy - 12.0, &bars, None)?;
    let tools = &tallies.tools;
    let cy = ui.card(right_x, y, right_w, CHART_H, t("Tools"), "", calls(Filter::All).map(|a| (t("All calls"), a)))?;
    ui.ranked(
        right_x + 16.0,
        cy,
        right_w - 32.0,
        tools,
        8,
        |v| format!("{v:.0}"),
        |name| {
            Some(Action::Go(Page::Calls {
                session: id.clone(),
                period: Period::All,
                filter: Filter::Tool(name.into()),
            }))
        },
    )?;
    y += CHART_H + GAP;

    // Disk usage and files.
    let rows = d.storage.len();
    let h = list_card_h(rows + 1).max(list_card_h(8));
    let total: u64 = d.storage.iter().map(|u| u.size.bytes).sum();
    let cy = ui.card(X0, y, left_w, h, t("Storage"), &f("{0} on disk", &[&stats::bytes(total)]), None)?;
    let mut ry = cy;
    for u in &d.storage {
        let action = if u.dir { Action::Open(u.path.clone()) } else { Action::Reveal(u.path.clone()) };
        storage_row(ui, X0 + 8.0, ry - 3.0, left_w - 16.0, &crate::i18n::tr(&u.label), "", u.size, total, action)?;
        ry += 26.0;
    }
    let changed = &tallies.changed;
    let items: Vec<(String, f64)> = changed.iter().map(|(n, v, _)| (n.clone(), *v)).collect();
    let cy = ui.card(right_x, y, right_w, h, t("Files changed"), "", files.map(|a| (t("All files"), a)))?;
    let distro = d.distro.clone();
    ui.ranked(
        right_x + 16.0,
        cy,
        right_w - 32.0,
        &items,
        ((h - 48.0) / 22.0) as usize,
        |v| format!("{v:.0}×"),
        |name| {
            changed.iter().find(|(n, _, _)| n == name).map(|(_, _, p)| Action::Reveal(local_path(p, distro.as_ref())))
        },
    )?;
    y += h + GAP;

    // The longest turns and the most frequent failures.
    let rows: Vec<(&Turn, Option<(&str, &str)>)> =
        tallies.turns.iter().filter(|t| t.secs > 0).map(|t| (t, None)).collect();
    y = turns_card(ui, v, y, &rows)?;
    let failures = &tallies.failures;
    let page = |filter: Filter| Page::Calls { session: Some(d.id.clone()), period: Period::All, filter };
    y = failures_card(ui, v, y, failures, &page)?;
    Ok(y - GAP)
}

/// Names with how often each comes, the most frequent first: `fork ×5 · Explore ×2`.
fn counted<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let mut by: Vec<(&str, u32)> = Vec::new();
    for name in names.filter(|n| !n.is_empty()) {
        match by.iter_mut().find(|(n, _)| *n == name) {
            Some((_, count)) => *count += 1,
            None => by.push((name, 1)),
        }
    }
    by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let parts: Vec<String> = by
        .iter()
        .map(|(name, count)| if *count > 1 { format!("{name} ×{count}") } else { (*name).to_string() })
        .collect();
    parts.join(" · ")
}

/// What a session's subagents did, summed up, as rows of a card: how many, of which
/// types, on which models, how long they ran, what they wrote. None for a session without.
fn agent_rows(d: &SessionData) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if !d.agents.is_empty() {
        let running = d.agents.iter().filter(|a| a.running).count();
        let mut started = num(d.agents.len() as u64);
        if running > 0 {
            started = format!("{started} · {}", f("{0} running", &[&running]));
        }
        rows.push((t("Started"), started));
        rows.push((t("Types"), counted(d.agents.iter().map(|a| a.kind.as_str()))));
        rows.push((t("Models"), counted(d.agents.iter().filter_map(|a| a.model.as_deref()))));
        // Each from its first reply to its last; agents that ran side by side count each.
        let ran: Vec<i64> = d.agents.iter().filter_map(|a| Some((a.last_at? - a.first_at?).max(0))).collect();
        rows.push((t("Ran for"), f("{0} in total", &[&duration(ran.iter().sum())])));
        rows.push((t("Longest"), duration(ran.iter().copied().max().unwrap_or(0))));
        rows.retain(|(_, value)| !value.is_empty());
    }
    // A kept session has its agents' replies, not the agents themselves.
    if d.summary.agent_output > 0 {
        rows.push((t("Tokens written"), tokens(d.summary.agent_output)));
        rows.push((t("API value"), stats::dollars(d.summary.agent_cost)));
    }
    rows
}

/// A card of rows, each a label and its value; `empty` when there are none.
fn rows_card(ui: &Ui, at: (f32, f32, f32, f32), title: &str, empty: &str, rows: &[(&str, String)]) -> Result<()> {
    let (x, y, w, h) = at;
    let cy = ui.card(x, y, w, h, title, "", None)?;
    let Some(c) = ui.c else { return Ok(()) };
    if rows.is_empty() {
        let n = ui.text(empty, 12.0, false, None)?;
        c.text(&n, x + 16.0, cy, ui.pal.text_dim);
        return Ok(());
    }
    let mut ly = cy;
    for (label, value) in rows {
        let l = ui.text(label, 12.0, false, None)?;
        let v = ui.text(value, 12.0, true, Some(w - 48.0 - l.width))?;
        c.text(&l, x + 16.0, ly, ui.pal.text_dim);
        c.text(&v, x + w - 16.0 - v.width, ly, ui.pal.text);
        ly += 26.0;
    }
    Ok(())
}

/// What a session's page shows besides its sums, worked out from its transcript's numbers.
struct SessionTallies {
    /// Activity per hour, the active hours only.
    hourly: BTreeMap<i64, Activity>,
    /// Calls per tool, most first.
    tools: Vec<(String, f64)>,
    /// Files changed: shown name, edits, path; most edited first.
    changed: Vec<(String, f64, String)>,
    /// The five longest turns.
    turns: Vec<Turn>,
    /// The five most frequent failures.
    failures: Vec<stats::Failure>,
}

fn session_tallies(s: &stats::Stats) -> SessionTallies {
    let mut hourly: BTreeMap<i64, Activity> = BTreeMap::new();
    let hour = |t: i64| t - t.rem_euclid(HOUR);
    for p in &s.prompts {
        hourly.entry(hour(p.at)).or_default().prompts += 1;
    }
    for r in &s.replies {
        let a = hourly.entry(hour(r.at)).or_default();
        a.replies += 1;
        a.output += r.tokens.output;
        a.cost += s.reply_cost(r);
    }
    for c in &s.calls {
        let a = hourly.entry(hour(c.at)).or_default();
        a.tool_calls += 1;
        a.errors += u32::from(c.error.is_some() && !c.declined());
    }
    let mut tools: Vec<(String, f64)> = Vec::new();
    for c in &s.calls {
        match tools.iter_mut().find(|(k, _)| *k == c.tool) {
            Some((_, n)) => *n += 1.0,
            None => tools.push((c.tool.clone(), 1.0)),
        }
    }
    tools.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut edits: Vec<(String, f64)> = Vec::new();
    // Edits that went through: one that failed or was declined changed nothing.
    for c in s.calls.iter().filter(|c| c.edits() && c.error.is_none()) {
        if let Some(p) = &c.path {
            // One file, however its path was written (C:\a.rs, c:/a.rs).
            let same = |k: &String| k.replace('\\', "/").eq_ignore_ascii_case(&p.replace('\\', "/"));
            match edits.iter_mut().find(|(k, _)| same(k)) {
                Some((_, n)) => *n += 1.0,
                None => edits.push((p.clone(), 1.0)),
            }
        }
    }
    let paths: Vec<&str> = edits.iter().map(|(p, _)| p.as_str()).collect();
    let names = distinct_names(&paths);
    let mut changed: Vec<(String, f64, String)> =
        names.into_iter().zip(&edits).map(|(name, (path, n))| (name, *n, path.clone())).collect();
    changed.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut turns = s.turns();
    turns.sort_by_key(|t| std::cmp::Reverse(t.secs));
    turns.truncate(5);
    let mut failures = stats::failures(s.calls.iter());
    failures.truncate(5);
    SessionTallies { hourly, tools, changed, turns, failures }
}

/// A name to show for each path: the file's name, with as many folders in
/// front as it takes to tell files of the same name apart (`ui/mod.rs`,
/// `core/mod.rs`). A click finds the file by this name.
fn distinct_names(paths: &[&str]) -> Vec<String> {
    let parts: Vec<Vec<&str>> =
        paths.iter().map(|p| p.split(['/', '\\']).filter(|s| !s.is_empty()).collect()).collect();
    let name =
        |parts: &[&str], depth: usize| parts.get(parts.len().saturating_sub(depth)..).unwrap_or_default().join("/");
    let mut depth = vec![1usize; paths.len()];
    loop {
        let names: Vec<String> = parts.iter().zip(&depth).map(|(p, d)| name(p, *d)).collect();
        let mut grew = false;
        for i in 0..names.len() {
            let twin = names.iter().enumerate().any(|(j, n)| j != i && *n == names[i]);
            if twin && depth[i] < parts[i].len() {
                depth[i] += 1;
                grew = true;
            }
        }
        if !grew {
            return names;
        }
    }
}

/// Most subagents the map shows.
const MAP_AGENTS: usize = 12;

/// The session and the subagents it started, as a tree: the session on the
/// left, one box per agent on the right (a blue dot while it runs); hover a
/// box for its numbers. Returns where the card ends.
fn agent_map(ui: &Ui, v: &View, d: &SessionData, title: &str, y: f32) -> Result<f32> {
    let pal = ui.pal;
    let s = &d.stats;
    let running = d.agents.iter().filter(|a| a.running).count();
    let mut note = plural(d.agents.len() as u64, "{0} agent", "{0} agents");
    if running > 0 {
        note = format!("{note} · {}", f("{0} running", &[&running]));
    }
    // The first few, or all of them (the page scrolls).
    let all = v.order.all_agents || d.agents.len() <= MAP_AGENTS;
    let shown = d.agents.get(..if all { d.agents.len() } else { MAP_AGENTS }).unwrap_or(d.agents.as_slice());
    let (node_h, node_gap) = (42.0, 6.0);
    let rows_h = shown.len() as f32 * (node_h + node_gap) - node_gap;
    let more = d.agents.len() - shown.len();
    let h = 12.0 + 18.0 + 10.0 + rows_h + if more > 0 { 28.0 } else { 0.0 } + 16.0;
    let link = match (more > 0, d.agents.len() > MAP_AGENTS) {
        (true, _) => Some((t("Show all"), Action::AllAgents(true))),
        (false, true) => Some((t("Show fewer"), Action::AllAgents(false))),
        _ => None,
    };
    let top = ui.card(X0, y, W, h, t("Agent map"), &note, link)?;
    let (lx, lw) = (X0 + 16.0, W * 0.38);
    let (cx, cw) = (X0 + W * 0.5, W * 0.5 - 16.0);
    let mid_of = |i: usize| top + i as f32 * (node_h + node_gap) + node_h / 2.0;
    let main_y = (mid_of(0) + mid_of(shown.len() - 1)) / 2.0 - node_h / 2.0;
    let now = v.m.now;

    // A box: dot, a bold first line and a dim second line. Returns whether it's hovered.
    let node = |x: f32, y: f32, w: f32, dot: Color, first: &str, second: &str| -> Result<bool> {
        let hovered = ui.hit(x, y, w, node_h, Action::Hover);
        let Some(c) = ui.c else { return Ok(hovered) };
        c.fill_round_rect(x, y, w, node_h, 6.0, if hovered { pal.card_hover } else { pal.window });
        c.stroke_round_rect(x + 0.5, y + 0.5, w - 1.0, node_h - 1.0, 6.0, 1.0, pal.stroke);
        c.fill_circle(x + 14.0, y + 13.0, 3.5, dot);
        let a = ui.text(first, 12.5, true, Some(w - 36.0))?;
        let b = ui.text(second, 11.0, false, Some(w - 36.0))?;
        c.text(&a, x + 26.0, y + 5.0, pal.text);
        c.text(&b, x + 26.0, y + 7.0 + a.height, pal.text_dim);
        Ok(hovered)
    };

    // The connecting lines first, under the boxes.
    if let Some(c) = ui.c {
        let line = pal.text_dim.with_alpha(0.35);
        let jx = (lx + lw + cx) / 2.0;
        c.fill_rect(lx + lw, main_y + node_h / 2.0, jx - lx - lw, 1.0, line);
        c.fill_rect(jx, mid_of(0), 1.0, mid_of(shown.len() - 1) - mid_of(0) + 1.0, line);
        for i in 0..shown.len() {
            c.fill_rect(jx, mid_of(i), cx - jx, 1.0, line);
        }
    }

    let model = s.main_model().unwrap_or_default();
    let context = d.context.last().map_or(0, |c| c.1);
    let main_line = f("{0} · {1} in context", &[&model, &tokens(context)]);
    node(lx, main_y, lw, pal.idle, title, &main_line)?;

    for (i, a) in shown.iter().enumerate() {
        let secs = a.last_at.zip(a.first_at).map_or(0, |(l, f)| l - f);
        let line = f("{0} · {1} written · {2} calls", &[&duration(secs), &tokens(a.output), &a.tool_calls]);
        let dot = if a.running { pal.working } else { pal.idle };
        let y = mid_of(i) - node_h / 2.0;
        if node(cx, y, cw, dot, &a.description, &line)? {
            let mut lines = Vec::new();
            if !a.kind.is_empty() {
                lines.push(f("Type: {0}", &[&a.kind]));
            }
            if let Some(m) = &a.model {
                lines.push(f("Model: {0}", &[m]));
            }
            lines.push(f("{0} tokens written, {1} in context at the end", &[&tokens(a.output), &tokens(a.context)]));
            lines.push(f("{0} tool calls, {1} failed", &[&a.tool_calls, &a.errors]));
            lines.push(f("API value: {0}", &[&stats::dollars(a.cost)]));
            if let Some(start) = a.first_at {
                lines.push(f("Started {0}, ran {1}", &[&when(start, now), &duration(secs)]));
            }
            lines.push(if a.running { t("Still running").to_string() } else { t("Finished").to_string() });
            ui.tip(cx + cw / 2.0, y, a.description.clone(), lines);
        }
    }
    if more > 0 {
        // A link under the last box that shows the rest.
        let n = ui.text(&f("Show {0} more", &[&more]), 12.0, false, None)?;
        let (lx, ly) = (cx, top + rows_h + 8.0);
        let hovered = ui.hit(lx - 6.0, ly - 4.0, n.width + 12.0, n.height + 8.0, Action::AllAgents(true));
        if let Some(c) = ui.c {
            if hovered {
                c.fill_round_rect(lx - 6.0, ly - 4.0, n.width + 12.0, n.height + 8.0, 5.0, pal.row_hover);
            }
            c.text(&n, lx, ly, if hovered { pal.text } else { pal.accent });
        }
    }
    Ok(y + h)
}

/// What a session loads, as rows for [`setup_card`].
fn setup_rows(s: &Setup) -> Vec<(&'static str, String)> {
    let mcp = if s.mcp_tools > 0 {
        f("{0} · {1} tools", &[&s.mcp_servers, &s.mcp_tools])
    } else {
        num(s.mcp_servers.into())
    };
    vec![
        (t("Skills"), num(s.skills.into())),
        (t("MCP servers"), mcp),
        (t("Agent types"), num(s.agents.into())),
        (t("Instruction files"), num(s.instruction_files.into())),
        (t("Memory index"), if s.memory { t("loaded") } else { t("not loaded") }.to_string()),
    ]
}

/// The "Setup" card: what is loaded, one row each.
fn setup_card(ui: &Ui, x: f32, y: f32, w: f32, note: &str, rows: &[(&str, String)]) -> Result<()> {
    let cy = ui.card(x, y, w, CHART_H, t("Setup"), note, None)?;
    let Some(c) = ui.c else { return Ok(()) };
    if rows.is_empty() {
        let n = ui.text(t("Not in this transcript."), 12.0, false, None)?;
        c.text(&n, x + 16.0, cy, ui.pal.text_dim);
        return Ok(());
    }
    let mut ly = cy;
    for (label, value) in rows {
        let l = ui.text(label, 12.0, false, None)?;
        let v = ui.text(value, 12.0, true, Some(w * 0.55))?;
        c.text(&l, x + 16.0, ly, ui.pal.text_dim);
        c.text(&v, x + w - 16.0 - v.width, ly, ui.pal.text);
        ly += 26.0;
    }
    Ok(())
}

/// The starting context split into its parts: estimates of what the
/// transcript lists, and the rest of the first reply's context.
fn starting_card(ui: &Ui, x: f32, y: f32, w: f32, setup: &Setup, context: Option<u64>) -> Result<()> {
    let pal = ui.pal;
    let note = context.map_or(String::new(), |c| f("{0} before the first reply", &[&tokens(c)]));
    let cy = ui.card(x, y, w, CHART_H, t("Starting context"), &note, None)?;
    let parts = setup.parts(context.unwrap_or(0));
    let Some(c) = ui.c else { return Ok(()) };
    if parts.is_empty() {
        let n = ui.text(t("Not in this transcript."), 12.0, false, None)?;
        c.text(&n, x + 16.0, cy, pal.text_dim);
        return Ok(());
    }
    let color = |p: Part| match p {
        Part::System => pal.accent,
        Part::Skills => pal.plan_to,
        Part::Mcp => pal.codex,
        Part::Agents => pal.done,
        Part::Instructions => pal.plan_from,
        Part::Memory => pal.needs_input,
        Part::Rest => pal.text_dim.with_alpha(0.35),
    };
    let total = parts.iter().map(|p| p.1).sum::<u64>().max(1) as f32;
    let (bx, bw) = (x + 16.0, w - 32.0);
    let mut px = bx;
    c.fill_round_rect(bx, cy, bw, 10.0, 5.0, pal.track);
    for &(p, v) in &parts {
        let pw = bw * v as f32 / total;
        if pw >= 1.0 {
            c.fill_rect(px, cy, pw, 10.0, color(p));
        }
        px += pw;
    }
    let mut ly = cy + 22.0;
    for &(p, v) in &parts {
        c.fill_circle(bx + 4.0, ly + 8.0, 4.0, color(p));
        let n = ui.text(t(p.label()), 12.0, false, None)?;
        let value = ui.text(&format!("~{}", tokens(v)), 12.0, true, None)?;
        let share = ui.text(&format!("{:.0}%", v as f32 / total * 100.0), 12.0, false, None)?;
        c.text(&n, bx + 14.0, ly, pal.text_dim);
        c.text(&share, bx + bw - share.width, ly, pal.text_dim);
        c.text(&value, bx + bw - 44.0 - value.width, ly, pal.text);
        ly += 22.0;
    }
    Ok(())
}

fn token_mix(ui: &Ui, x: f32, y: f32, w: f32, sum: &stats::Summary) -> Result<()> {
    let pal = ui.pal;
    let t = sum.tokens;
    let cy = ui.card(x, y, w, CHART_H, i18n::t("Tokens"), &f("{0} thinking", &[&tokens(t.thinking)]), None)?;
    let parts = [
        (i18n::t("Cache read"), t.cache_read, pal.accent),
        (i18n::t("Cache write"), t.cache_write, pal.warn),
        (i18n::t("Input"), t.input, pal.text_dim),
        (i18n::t("Output"), t.output, pal.done),
        (i18n::t("By agents"), sum.agent_output, pal.done.with_alpha(0.5)),
    ];
    let total = parts.iter().map(|p| p.1).sum::<u64>().max(1) as f32;
    let Some(c) = ui.c else { return Ok(()) };
    let (bx, bw) = (x + 16.0, w - 32.0);
    let mut px = bx;
    c.fill_round_rect(bx, cy, bw, 10.0, 5.0, pal.track);
    for (_, v, color) in parts {
        let pw = bw * v as f32 / total;
        if pw >= 1.0 {
            c.fill_rect(px, cy, pw, 10.0, color);
        }
        px += pw;
    }
    let mut ly = cy + 24.0;
    for (name, v, color) in parts {
        c.fill_circle(bx + 4.0, ly + 8.0, 4.0, color);
        let n = ui.text(name, 12.0, false, None)?;
        let value = ui.text(&tokens(v), 12.0, true, None)?;
        let share = ui.text(&format!("{:.0}%", v as f32 / total * 100.0), 12.0, false, None)?;
        c.text(&n, bx + 14.0, ly, pal.text_dim);
        c.text(&share, bx + bw - share.width, ly, pal.text_dim);
        c.text(&value, bx + bw - 44.0 - value.width, ly, pal.text);
        ly += 24.0;
    }
    Ok(())
}

/// A file or folder with its size, share and an open button.
#[allow(clippy::too_many_arguments, reason = "position, width, texts, size, share, action")]
fn storage_row(
    ui: &Ui,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    note: &str,
    size: stats::Size,
    total: u64,
    action: Action,
) -> Result<()> {
    let h = 24.0;
    let hovered = ui.hit(x, y, w, h, action);
    if !ui.shown(y, h) {
        return Ok(());
    }
    let Some(c) = ui.c else { return Ok(()) };
    let pal = ui.pal;
    if hovered {
        c.fill_round_rect(x, y, w, h, 5.0, pal.row_hover);
    }
    let icon = ui.g.glyph(FOLDER, 12.0)?;
    let value = ui.text(&stats::bytes(size.bytes), 12.0, true, None)?;
    let files = ui.text(&plural(size.files, "{0} file", "{0} files"), 12.0, false, None)?;
    let label_w = w * 0.40;
    let l = ui.text(label, 12.0, false, Some(label_w))?;
    c.text(&l, x + 8.0, y + (h - l.height) / 2.0, if hovered { pal.accent } else { pal.text });
    if !note.is_empty() {
        let n = ui.text(note, 11.0, false, Some(label_w - l.width - 8.0))?;
        c.text(&n, x + 8.0 + l.width + 8.0, y + (h - n.height) / 2.0, pal.text_dim);
    }
    let right = x + w - 8.0 - icon.width - 10.0;
    c.text(
        &icon,
        x + w - 8.0 - icon.width,
        y + (h - icon.height) / 2.0 + 1.0,
        if hovered { pal.accent } else { pal.text_dim },
    );
    c.text(&value, right - value.width, y + (h - value.height) / 2.0, pal.text);
    let files_right = right - 76.0;
    c.text(&files, files_right - files.width, y + (h - files.height) / 2.0, pal.text_dim);
    // The share of the total, where there's room for it.
    let bar_x = x + 8.0 + label_w + 12.0;
    let bar_w = files_right - 100.0 - bar_x;
    if bar_w >= 80.0 {
        let mid = y + h / 2.0;
        c.fill_round_rect(bar_x, mid - 3.0, bar_w, 6.0, 3.0, pal.track.with_alpha(0.6));
        let share = if total > 0 { size.bytes as f32 / total as f32 } else { 0.0 };
        c.fill_round_rect(bar_x, mid - 3.0, (bar_w * share).max(6.0), 6.0, 3.0, pal.accent);
    }
    Ok(())
}

/// A tool call in a list, with its error; with `session`, also which
/// session it belongs to (and clicking opens it).
fn call_row(
    ui: &Ui,
    now: i64,
    x: f32,
    y: f32,
    w: f32,
    call: &stats::Call,
    session: Option<&(String, String)>,
) -> Result<()> {
    let h = if call.error.is_some() { 48.0 } else { 30.0 };
    let action = session.map_or(Action::Hover, |(id, _)| Action::Go(Page::Session(id.clone())));
    let hovered = ui.hit(x, y, w, h, action);
    if !ui.shown(y, h) {
        return Ok(());
    }
    let Some(c) = ui.c else { return Ok(()) };
    let pal = ui.pal;
    if hovered {
        c.fill_round_rect(x, y, w, h, 6.0, pal.row_hover);
    }
    let dot = match () {
        _ if call.declined() => pal.warn,
        _ if call.error.is_some() => pal.crit,
        _ => pal.accent.with_alpha(0.7),
    };
    let time = ui.text(&when(call.at, now), 12.0, false, Some(84.0))?;
    c.text(&time, x + 10.0, y + 7.0, pal.text_dim);
    c.fill_circle(x + 104.0, y + 15.0, 3.5, dot);
    let tool = ui.text(&call.tool, 13.0, true, Some(126.0))?;
    c.text(&tool, x + 114.0, y + 6.0, pal.text);
    let dx = x + 250.0;
    let session_w = if session.is_some() { 170.0 } else { 0.0 };
    let detail_w = x + w - 10.0 - session_w - dx;
    let detail = ui.text(&call.detail, 12.5, false, Some(detail_w))?;
    c.text(&detail, dx, y + 7.0, pal.text);
    if let Some((_, title)) = session {
        let t = ui.text(title, 12.0, false, Some(session_w - 12.0))?;
        c.text(&t, x + w - 10.0 - t.width, y + 7.0, if hovered { pal.accent } else { pal.text_dim });
    }
    if let Some(e) = &call.error {
        let color = if call.declined() { pal.warn } else { pal.crit };
        let et = ui.text(e, 12.0, false, Some(x + w - 10.0 - dx))?;
        c.text(&et, dx, y + 26.0, color);
    }
    if hovered {
        let mut lines = vec![call.detail.clone()];
        if let Some(e) = &call.error {
            lines.push(if call.declined() { f("Declined or blocked: {0}", &[e]) } else { f("Error: {0}", &[e]) });
        }
        let title = match session {
            Some((_, t)) => format!("{} · {}", call.tool, t),
            None => call.tool.clone(),
        };
        let mx = ui.mouse.map_or(dx, |(mx, _)| mx);
        ui.tip(mx, y + 2.0, title, lines);
    }
    Ok(())
}

fn calls_page(
    ui: &Ui,
    v: &View,
    l: &Listing<CallRow>,
    (session, period): (Option<&String>, Period),
    filter: &Filter,
    y: f32,
) -> Result<f32> {
    let failed = |r: &&CallRow| r.call.error.is_some() && !r.call.declined();
    let title = match filter {
        Filter::All => t("Tool calls").to_string(),
        Filter::Failed => t("Failed tool calls").to_string(),
        Filter::Tool(tool) => f("{0} calls", &[tool]),
        Filter::Error(_) => t("Failures like this").to_string(),
    };
    let tools_w = list_tools(ui, y, v.search)?;
    let mut y = header(ui, y, W - tools_w, &title, &l.title)?;
    let page = |filter: Filter| Page::Calls { session: session.cloned(), period, filter };
    // Worked out once per data, filter, search and order, not for every frame.
    let key = ("calls", format!("{filter:?}"), v.search, v.order.calls_oldest_first);
    let list = memo(&key, || {
        let mut counts: Vec<(String, u64)> = Vec::new();
        for r in &l.items {
            match counts.iter_mut().find(|(k, _)| *k == r.call.tool) {
                Some((_, n)) => *n += 1,
                None => counts.push((r.call.tool.clone(), 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut groups = Vec::new();
        if matches!(filter, Filter::Failed | Filter::Error(_)) {
            groups = stats::failures(l.items.iter().map(|r| &r.call));
            groups.truncate(6);
        }
        CallList {
            counts,
            failed: l.items.iter().filter(failed).count(),
            groups,
            rows: call_rows(l, filter, v.search, &v.order),
        }
    });
    let counts = &list.counts;
    let mut chips = vec![
        (f("All  {0}", &[&num(l.items.len() as u64)]), *filter == Filter::All, Action::Swap(page(Filter::All))),
        (f("Failed  {0}", &[&num(list.failed as u64)]), *filter == Filter::Failed, Action::Swap(page(Filter::Failed))),
    ];
    for (tool, n) in counts.iter().take(12) {
        let tool_filter = Filter::Tool(tool.clone());
        chips.push((format!("{tool}  {}", num(*n)), *filter == tool_filter, Action::Swap(page(tool_filter))));
    }
    y = ui.chips(X0, y, W, &chips)?;
    let oldest = v.order.calls_oldest_first;
    y = ui.sort_row(
        y,
        t("Order"),
        &[
            (t("Newest first").to_string(), !oldest, Action::CallsOldestFirst(false)),
            (t("Oldest first").to_string(), oldest, Action::CallsOldestFirst(true)),
        ],
    )?;
    {
        // The failures grouped by kind; click one to list only those.
        let groups = &list.groups;
        if !groups.is_empty() {
            let h = 12.0 + 18.0 + 10.0 + groups.len() as f32 * 46.0 + 8.0;
            let cy = ui.card(X0, y, W, h, t("Common failures"), t("click one to list only those"), None)?;
            let mut ry = cy - 4.0;
            for g in groups {
                let selected = *filter == Filter::Error(g.signature.clone());
                let target = if selected { Filter::Failed } else { Filter::Error(g.signature.clone()) };
                failure_row(ui, v.m.now, X0 + 6.0, ry, W - 12.0, g, selected, Action::Swap(page(target)))?;
                ry += 46.0;
            }
            y += h + GAP;
        }
    }
    if list.rows.is_empty() {
        let t = ui.text(t("Nothing here."), 13.0, false, None)?;
        if let Some(c) = ui.c {
            c.text(&t, X0, y, ui.pal.text_dim);
        }
        return Ok(y + 24.0);
    }
    // A long list shows its start: every row is a clickable area, laid out
    // for each frame. The CSV has all of them.
    let rows = list.rows.iter().take(MAX_ROWS).filter_map(|i| l.items.get(*i));
    y = capped_note(ui, y, list.rows.len())?;
    let height = |r: &CallRow| if r.call.error.is_some() { 48.0 } else { 30.0 };
    let total: f32 = rows.clone().map(height).sum();
    if let Some(c) = ui.c {
        c.fill_round_rect(X0, y, W, total + 12.0, 8.0, ui.pal.card);
    }
    let mut ry = y + 6.0;
    for r in rows {
        call_row(ui, v.m.now, X0 + 6.0, ry, W - 12.0, &r.call, r.session.as_ref())?;
        ry += height(r);
    }
    Ok(ry + 6.0)
}

/// Most rows a list shows: each is a clickable area laid out for every
/// frame. The CSV has all of them.
const MAX_ROWS: usize = 2000;

/// Above a list longer than [`MAX_ROWS`], a line that says so; returns where the list starts.
fn capped_note(ui: &Ui, y: f32, all: usize) -> Result<f32> {
    if all <= MAX_ROWS {
        return Ok(y);
    }
    let (shown, all) = (num(MAX_ROWS as u64), num(all as u64));
    let note = f("Showing the first {0} of {1}. Search or filter to narrow it down; the CSV has all.", &[&shown, &all]);
    let note = ui.text(&note, 12.0, false, Some(W))?;
    if let Some(c) = ui.c {
        c.text(&note, X0, y, ui.pal.text_dim);
    }
    Ok(y + note.height + 8.0)
}

/// What the tool call list shows, worked out from the page's data.
struct CallList {
    /// Calls per tool, most first.
    counts: Vec<(String, u64)>,
    failed: usize,
    /// The failures by kind (on the failed lists).
    groups: Vec<stats::Failure>,
    /// The listed calls in order, as indices into the data's items.
    rows: Vec<usize>,
}

/// Search field and CSV button at the right of a list page's title; returns their width.
fn list_tools(ui: &Ui, y: f32, search: &str) -> Result<f32> {
    let w = ui.search_box(X0 + W, y + 2.0, search)?;
    let b = ui.button(X0 + W - w - 8.0, y + 2.0, DOWNLOAD, "CSV", Action::Export)?;
    Ok(w + 8.0 + b + 12.0)
}

/// Case-insensitive match of `search` in any of `fields`.
fn matches(search: &str, fields: &[&str]) -> bool {
    search.is_empty() || {
        let s = search.to_lowercase();
        fields.iter().any(|f| f.to_lowercase().contains(&s))
    }
}

/// The sort pills of a list: the selected one shows its direction.
fn sort_items<T: Copy + PartialEq>(
    all: &[T],
    selected: T,
    desc: bool,
    label: impl Fn(T) -> &'static str,
    action: impl Fn(T) -> Action,
) -> Vec<(String, bool, Action)> {
    all.iter()
        .map(|&s| {
            let on = s == selected;
            let text =
                if on { format!("{} {}", label(s), if desc { "↓" } else { "↑" }) } else { label(s).to_string() };
            (text, on, action(s))
        })
        .collect()
}

/// The calls the list shows, in order, as indices into `l.items`.
fn call_rows(l: &Listing<CallRow>, filter: &Filter, search: &str, order: &Order) -> Vec<usize> {
    let mut rows: Vec<usize> = l
        .items
        .iter()
        .enumerate()
        .filter(|(_, r)| match filter {
            Filter::All => true,
            Filter::Failed => r.call.error.is_some() && !r.call.declined(),
            Filter::Tool(t) => r.call.tool == *t,
            Filter::Error(sig) => r.call.signature().as_ref() == Some(sig),
        })
        .filter(|(_, r)| {
            let session = r.session.as_ref().map_or("", |(_, t)| t.as_str());
            matches(search, &[&r.call.tool, &r.call.detail, r.call.error.as_deref().unwrap_or(""), session])
        })
        .map(|(i, _)| i)
        .collect();
    // Listed newest first.
    if order.calls_oldest_first {
        rows.reverse();
    }
    rows
}

/// Where each of `picked` (references into `all`) is in `all`.
fn indices<T>(all: &[T], picked: &[&T]) -> Vec<usize> {
    let (base, size) = (all.as_ptr() as usize, std::mem::size_of::<T>().max(1));
    picked.iter().map(|r| (*r as *const T as usize).wrapping_sub(base) / size).collect()
}

fn file_rows<'a>(l: &'a Listing<FileRow>, search: &str, order: &Order) -> Vec<&'a FileRow> {
    let mut rows: Vec<&FileRow> = l.items.iter().filter(|f| matches(search, &[&f.path])).collect();
    rows.sort_by(|a, b| {
        let o = match order.files {
            FileSort::Edits => a.edits.cmp(&b.edits).then(a.reads.cmp(&b.reads)),
            FileSort::Lines => (a.added + a.removed).cmp(&(b.added + b.removed)),
            FileSort::Reads => a.reads.cmp(&b.reads),
            FileSort::Recent => a.last_at.cmp(&b.last_at),
            FileSort::Name => file_name(&a.path).to_lowercase().cmp(&file_name(&b.path).to_lowercase()),
        };
        if order.files_desc { o.reverse() } else { o }
    });
    rows
}

/// The projects in the order the list shows them (and the CSV has them).
fn project_rows<'a>(l: &'a [stats::ProjectInfo], order: &Order) -> Vec<&'a stats::ProjectInfo> {
    let mut rows: Vec<&stats::ProjectInfo> = l.iter().collect();
    rows.sort_by(|a, b| {
        let o = match order.projects {
            ProjectSort::Recent => a.last_at.cmp(&b.last_at),
            ProjectSort::Sessions => a.sessions.cmp(&b.sessions),
            ProjectSort::Prompts => a.prompts.cmp(&b.prompts),
            ProjectSort::Tokens => a.output.cmp(&b.output),
            ProjectSort::Cost => a.cost.total_cmp(&b.cost),
            ProjectSort::Failures => a.errors.cmp(&b.errors),
            ProjectSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        // Equal ones: the one used last first, then by name, whichever way the sort goes.
        let o = if order.projects_desc { o.reverse() } else { o };
        o.then(b.last_at.cmp(&a.last_at)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

fn session_rows<'a>(l: &'a [SessionInfo], archived: Option<bool>, search: &str, order: &Order) -> Vec<&'a SessionInfo> {
    let mut rows: Vec<&SessionInfo> = l
        .iter()
        .filter(|s| archived.is_none_or(|a| s.archived == a))
        .filter(|s| matches(search, &[&s.title, &s.project, &s.id, s.distro.as_deref().unwrap_or("")]))
        .collect();
    let length = |s: &SessionInfo| s.last_at.zip(s.first_at).map_or(0, |(l, f)| l - f);
    rows.sort_by(|a, b| {
        let o = match order.sessions {
            SessionSort::Recent => a.last_at.cmp(&b.last_at),
            SessionSort::Prompts => a.prompts.cmp(&b.prompts),
            SessionSort::Tokens => a.output.cmp(&b.output),
            SessionSort::Cost => a.cost.total_cmp(&b.cost),
            SessionSort::Size => a.size.bytes.cmp(&b.size.bytes),
            SessionSort::Length => length(a).cmp(&length(b)),
            SessionSort::Failures => a.errors.cmp(&b.errors),
            SessionSort::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        };
        let o = o.then(a.last_at.cmp(&b.last_at));
        if order.sessions_desc { o.reverse() } else { o }
    });
    rows
}

/// A CSV field, quoted when needed. Text that a spreadsheet would run as a
/// formula (it starts with `=`, `+`, `-` or `@`; titles and tool details
/// come from transcripts) gets a leading apostrophe; numbers stay numbers.
fn csv_field(s: &str) -> String {
    let formula = s.starts_with(['=', '+', '-', '@', '\t', '\r']) && s.parse::<f64>().is_err();
    let s = if formula { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

/// CSV text (UTF-8 with a byte order mark, so Excel reads it right).
fn csv(header: &[&str], rows: impl Iterator<Item = Vec<String>>) -> String {
    let mut out = String::from("\u{FEFF}");
    out += &header.join(",");
    out += "\r\n";
    for row in rows {
        out += &row.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(",");
        out += "\r\n";
    }
    out
}

/// What the page lists, as a file name and CSV text.
fn export(page: &Page, data: &Data, search: &str, order: &Order, now: i64) -> Option<(String, String)> {
    let t = |at: i64| timefmt::rfc3339(at);
    let opt = |at: Option<i64>| at.map(timefmt::rfc3339).unwrap_or_default();
    let (kind, text) = match (page, data) {
        (Page::Overview(p), Data::Overview(o)) => (
            format!("overview-{}", p.label().replace(' ', "-").to_lowercase()),
            csv(
                &[
                    "start",
                    "prompts",
                    "replies",
                    "tokens_written",
                    "tokens_by_agents",
                    "tool_calls",
                    "failed_calls",
                    "api_value_usd",
                ],
                o.buckets.iter().map(|(at, a)| {
                    vec![
                        t(*at),
                        a.prompts.to_string(),
                        a.replies.to_string(),
                        a.output.to_string(),
                        a.agent_output.to_string(),
                        a.tool_calls.to_string(),
                        a.errors.to_string(),
                        format!("{:.4}", a.cost),
                    ]
                }),
            ),
        ),
        (Page::Calls { filter, .. }, Data::Calls(l)) => (
            "tool-calls".to_string(),
            csv(
                &["time", "session", "tool", "detail", "error", "declined"],
                call_rows(l, filter, search, order).into_iter().map(|i| &l.items[i]).map(|r| {
                    vec![
                        t(r.call.at),
                        r.session.as_ref().map_or(l.title.clone(), |(_, s)| s.clone()),
                        r.call.tool.clone(),
                        r.call.detail.clone(),
                        r.call.error.clone().unwrap_or_default(),
                        r.call.declined().to_string(),
                    ]
                }),
            ),
        ),
        (Page::Files { .. }, Data::Files(l)) => (
            "files".to_string(),
            csv(
                &["path", "edits", "lines_written", "lines_replaced", "reads", "last_used"],
                file_rows(l, search, order).into_iter().map(|f| {
                    vec![
                        f.path.clone(),
                        f.edits.to_string(),
                        f.added.to_string(),
                        f.removed.to_string(),
                        f.reads.to_string(),
                        t(f.last_at),
                    ]
                }),
            ),
        ),
        (Page::Sessions { archived }, Data::Sessions(l)) => (
            "sessions".to_string(),
            csv(
                &[
                    "id",
                    "title",
                    "project",
                    "wsl",
                    "first",
                    "last",
                    "prompts",
                    "tokens_written",
                    "api_value_usd",
                    "failed_calls",
                    "bytes",
                    "archived",
                ],
                session_rows(l, *archived, search, order).into_iter().map(|s| {
                    vec![
                        s.id.clone(),
                        s.title.clone(),
                        s.project.clone(),
                        s.distro.clone().unwrap_or_default(),
                        opt(s.first_at),
                        opt(s.last_at),
                        s.prompts.to_string(),
                        s.output.to_string(),
                        format!("{:.4}", s.cost),
                        s.errors.to_string(),
                        s.size.bytes.to_string(),
                        s.archived.to_string(),
                    ]
                }),
            ),
        ),
        (Page::Projects, Data::Projects(l)) => (
            "projects".to_string(),
            csv(
                &["project", "sessions", "prompts", "tokens_written", "api_value_usd", "failed_calls", "first", "last"],
                project_rows(l, order).into_iter().map(|p| {
                    vec![
                        p.name.clone(),
                        p.sessions.to_string(),
                        p.prompts.to_string(),
                        p.output.to_string(),
                        format!("{:.4}", p.cost),
                        p.errors.to_string(),
                        opt(p.first_at),
                        opt(p.last_at),
                    ]
                }),
            ),
        ),
        (Page::Project { name, .. }, Data::Overview(o)) => (
            format!("project-{}", name.replace(' ', "-").to_lowercase()),
            csv(
                &["model", "tokens_written", "tokens_read", "replies", "api_value_usd"],
                o.models.iter().map(|m| {
                    vec![
                        m.name.clone(),
                        m.output.to_string(),
                        m.input.to_string(),
                        m.replies.to_string(),
                        format!("{:.4}", m.cost),
                    ]
                }),
            ),
        ),
        (Page::Weeks, Data::Weeks(l)) => (
            "weeks".to_string(),
            csv(
                &["week_start", "sessions", "prompts", "tokens_written", "api_value_usd", "claude_worked_seconds"],
                l.iter().map(|w| {
                    vec![
                        t(w.start),
                        w.sessions.to_string(),
                        w.prompts.to_string(),
                        w.tokens.to_string(),
                        format!("{:.4}", w.cost),
                        w.working.to_string(),
                    ]
                }),
            ),
        ),
        (Page::Storage, Data::Storage(roots)) => (
            "storage".to_string(),
            csv(
                &["where", "folder", "path", "bytes", "files"],
                roots.iter().flat_map(|r| {
                    r.entries.iter().chain(&r.projects).map(|u| {
                        vec![
                            r.label.clone(),
                            u.label.clone(),
                            u.path.display().to_string(),
                            u.size.bytes.to_string(),
                            u.size.files.to_string(),
                        ]
                    })
                }),
            ),
        ),
        _ => return None,
    };
    let stamp: String = timefmt::rfc3339(now).chars().filter(char::is_ascii_digit).take(12).collect();
    // A project's name is part of it: only what a file's name can have (a `:` would
    // write into a stream of another file).
    let kind: String = kind.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect();
    Some((format!("trayward-{kind}-{stamp}.csv"), text))
}

fn files_page(ui: &Ui, v: &View, l: &Listing<FileRow>, y: f32) -> Result<f32> {
    let edited = l.items.iter().filter(|f| f.edits > 0).count() as u64;
    let subtitle = format!(
        "{} · {} · {}",
        l.title,
        f("{0} changed", &[&num(edited)]),
        f("{0} only read", &[&num(l.items.len() as u64 - edited)])
    );
    let tools_w = list_tools(ui, y, v.search)?;
    let mut y = header(ui, y, W - tools_w, t("Files"), &subtitle)?;
    let o = v.order;
    let sorts = sort_items(&FileSort::ALL, o.files, o.files_desc, FileSort::label, Action::SortFiles);
    y = ui.sort_row(y, t("Sort by"), &sorts)?;
    // Filtered and sorted once per data, search and order, not for every frame.
    let picked =
        memo(&("files", v.search, format!("{:?}", v.order)), || indices(&l.items, &file_rows(l, v.search, &v.order)));
    let items: Vec<&FileRow> = picked.iter().take(MAX_ROWS).filter_map(|i| l.items.get(*i)).collect();
    if items.is_empty() {
        let t = ui.text(t("No files read or changed."), 13.0, false, None)?;
        if let Some(c) = ui.c {
            c.text(&t, X0, y, ui.pal.text_dim);
        }
        return Ok(y + 24.0);
    }
    let h = 30.0;
    y = capped_note(ui, y, picked.len())?;
    if let Some(c) = ui.c {
        c.fill_round_rect(X0, y, W, items.len() as f32 * h + 12.0, 8.0, ui.pal.card);
    }
    y += 6.0;
    let pal = ui.pal;
    for f in items {
        let (x, w) = (X0 + 6.0, W - 12.0);
        let hovered = ui.hit(x, y, w, h, Action::Reveal(local_path(&f.path, f.distro.as_ref())));
        if ui.shown(y, h) {
            if let Some(c) = ui.c {
                if hovered {
                    c.fill_round_rect(x, y, w, h, 6.0, pal.row_hover);
                }
                let name = ui.text(file_name(&f.path), 13.0, true, Some(220.0))?;
                c.text(&name, x + 10.0, y + 6.0, if hovered { pal.accent } else { pal.text });
                let cols_x = x + w - 330.0;
                let dir =
                    ui.text(parent(&f.path), 12.0, false, Some(cols_x - (x + 10.0 + name.width + 10.0) - 10.0))?;
                c.text(&dir, x + 10.0 + name.width + 10.0, y + 7.0, pal.text_dim);
                let mut cx = cols_x;
                if f.edits > 0 {
                    let e = ui.text(&plural(f.edits.into(), "{0} edit", "{0} edits"), 12.0, false, None)?;
                    c.text(&e, cx, y + 7.0, pal.text);
                    let added = ui.text(&format!("+{}", num(f.added)), 12.0, false, None)?;
                    let removed = ui.text(&format!("−{}", num(f.removed)), 12.0, false, None)?;
                    // After the count, however long its words are.
                    let ax = cx + (e.width + 8.0).max(70.0);
                    c.text(&added, ax, y + 7.0, pal.done);
                    c.text(&removed, ax + added.width + 5.0, y + 7.0, pal.crit);
                }
                cx += 150.0;
                if f.reads > 0 {
                    let r = ui.text(&plural(f.reads.into(), "{0} read", "{0} reads"), 12.0, false, None)?;
                    c.text(&r, cx, y + 7.0, pal.text_dim);
                }
                let t = ui.text(&when(f.last_at, v.m.now), 12.0, false, None)?;
                let icon = ui.g.glyph(FOLDER, 12.0)?;
                c.text(&t, x + w - 34.0 - t.width, y + 7.0, pal.text_dim);
                c.text(&icon, x + w - 10.0 - icon.width, y + 9.0, if hovered { pal.accent } else { pal.text_dim });
            }
        }
        y += h;
    }
    Ok(y + 6.0)
}

fn sessions_page(ui: &Ui, v: &View, l: &[SessionInfo], archived: Option<bool>, y: f32) -> Result<f32> {
    let total_size: u64 = l.iter().map(|s| s.size.bytes).sum();
    let subtitle = format!(
        "{} · {} · {}",
        plural(l.len() as u64, "{0} session", "{0} sessions"),
        f("{0} on disk", &[&stats::bytes(total_size)]),
        t("past and archived sessions included")
    );
    let tools_w = list_tools(ui, y, v.search)?;
    let mut y = header(ui, y, W - tools_w, t("All sessions"), &subtitle)?;
    let n_archived = l.iter().filter(|s| s.archived).count() as u64;
    let chips = [
        (f("All  {0}", &[&num(l.len() as u64)]), archived.is_none(), Action::Swap(Page::Sessions { archived: None })),
        (
            f("Not archived  {0}", &[&num(l.len() as u64 - n_archived)]),
            archived == Some(false),
            Action::Swap(Page::Sessions { archived: Some(false) }),
        ),
        (
            f("Archived  {0}", &[&num(n_archived)]),
            archived == Some(true),
            Action::Swap(Page::Sessions { archived: Some(true) }),
        ),
    ];
    y = ui.chips(X0, y, W, &chips)?;
    let o = v.order;
    let sorts = sort_items(&SessionSort::ALL, o.sessions, o.sessions_desc, SessionSort::label, Action::SortSessions);
    y = ui.sort_row(y, t("Sort by"), &sorts)?;
    let picked = memo(&("sessions", archived, v.search, format!("{:?}", v.order)), || {
        indices(l, &session_rows(l, archived, v.search, &v.order))
    });
    let rows: Vec<&SessionInfo> = picked.iter().take(MAX_ROWS).filter_map(|i| l.get(*i)).collect();
    if rows.is_empty() {
        let t = ui.text(t("Nothing here."), 13.0, false, None)?;
        if let Some(c) = ui.c {
            c.text(&t, X0, y, ui.pal.text_dim);
        }
        return Ok(y + 24.0);
    }
    let y = capped_note(ui, y, picked.len())?;
    if let Some(c) = ui.c {
        c.fill_round_rect(X0, y, W, rows.len() as f32 * 50.0 + 12.0, 8.0, ui.pal.card);
    }
    let mut ry = y + 6.0;
    for s in rows {
        session_row(ui, v.m.now, X0 + 6.0, ry, W - 12.0, s)?;
        ry += 50.0;
    }
    Ok(ry + 6.0)
}

fn storage_page(ui: &Ui, roots: &[Storage], y: f32) -> Result<f32> {
    let total: u64 = roots.iter().map(|s| s.total.bytes).sum();
    let files: u64 = roots.iter().map(|s| s.total.files).sum();
    let subtitle =
        f("{0} in {1} · Claude Code's own folders", &[&stats::bytes(total), &plural(files, "{0} file", "{0} files")]);
    ui.button(X0 + W, y + 2.0, DOWNLOAD, "CSV", Action::Export)?;
    let mut y = header(ui, y, W - 150.0, t("Storage"), &subtitle)?;
    for s in roots {
        let rows = s.entries.len().min(14);
        let h = 12.0 + 18.0 + 10.0 + rows as f32 * 26.0 + 10.0;
        let note = format!("{} · {}", stats::bytes(s.total.bytes), s.dir.display());
        let cy = ui.card(X0, y, W, h, &s.label, &note, Some((t("Open"), Action::Open(s.dir.clone()))))?;
        let mut ry = cy - 3.0;
        for u in s.entries.iter().take(rows) {
            let action = if u.dir { Action::Open(u.path.clone()) } else { Action::Reveal(u.path.clone()) };
            // Known names get a description, with the name itself as a note.
            let (label, note) = match storage_label(&u.label) {
                Some(label) => (label, u.label.as_str()),
                None => (u.label.as_str(), ""),
            };
            storage_row(ui, X0 + 8.0, ry, W - 16.0, label, note, u.size, s.total.bytes, action)?;
            ry += 26.0;
        }
        y += h + GAP;
        if !s.projects.is_empty() {
            let rows = s.projects.len().min(12);
            let h = 12.0 + 18.0 + 10.0 + rows as f32 * 26.0 + 10.0;
            let projects_total: u64 = s.projects.iter().map(|p| p.size.bytes).sum();
            let title = f("Transcripts per project · {0}", &[&s.label]);
            let cy = ui.card(X0, y, W, h, &title, t("largest first"), None)?;
            let mut ry = cy - 3.0;
            for u in s.projects.iter().take(rows) {
                storage_row(
                    ui,
                    X0 + 8.0,
                    ry,
                    W - 16.0,
                    &u.label,
                    "",
                    u.size,
                    projects_total,
                    Action::Open(u.path.clone()),
                )?;
                ry += 26.0;
            }
            y += h + GAP;
        }
    }
    Ok(y - GAP)
}

pub struct StatsWindow {
    pub hwnd: Option<HWND>,
    pub page: Page,
    period: Period,
    model: Option<Model>,
    data: Data,
    layout: Layout,
    hover: Option<usize>,
    /// Mouse in window DIPs.
    mouse: Option<(f32, f32)>,
    in_zone: bool,
    scroll: f32,
    /// Where smooth scrolling is heading.
    target: f32,
    history: Vec<Page>,
    search: String,
    order: Order,
    copied: Option<String>,
    /// How far the thread that reads for the page has got (see `stats::report_to`).
    pub progress: std::sync::Arc<stats::Progress>,
}

/// Timer that animates scrolling.
pub const SCROLL_TIMER: usize = 1;
/// Timer that draws the page again while it's being read, to show how far that is.
pub const LOAD_TIMER: usize = 2;

fn scale_of(hwnd: HWND) -> f32 {
    // SAFETY: plain query on our own window.
    unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0
}

impl StatsWindow {
    pub fn new() -> StatsWindow {
        StatsWindow {
            hwnd: None,
            page: Page::Overview(Period::Week),
            period: Period::Week,
            model: None,
            data: Data::Loading,
            layout: Layout::default(),
            hover: None,
            mouse: None,
            in_zone: false,
            scroll: 0.0,
            target: 0.0,
            history: Vec::new(),
            search: String::new(),
            order: Order::default(),
            copied: None,
            progress: Default::default(),
        }
    }

    /// The page's data is being read: nothing to show yet but how far that is.
    fn loading(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        self.data = Data::Loading;
        new_data();
        self.progress.done.store(0, Relaxed);
        self.progress.total.store(0, Relaxed);
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window; setting the timer again restarts it.
            unsafe { SetTimer(Some(hwnd), LOAD_TIMER, 100, None) };
        }
    }

    fn loaded(&self) {
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window; no harm if the timer isn't set.
            let _ = unsafe { KillTimer(Some(hwnd), LOAD_TIMER) };
        }
    }

    pub fn visible(&self) -> bool {
        // SAFETY: plain query on our own window.
        self.hwnd.is_some_and(|h| unsafe { IsWindowVisible(h) }.as_bool())
    }

    /// The window's size for `dpi`, at most `max_h` pixels high (the work
    /// area's height); notes how high the content area is then.
    fn window_size(dpi: u32, max_h: i32) -> (i32, i32) {
        let s = dpi as f32 / 96.0;
        let mut r = RECT { left: 0, top: 0, right: (WIDTH * s).ceil() as i32, bottom: (HEIGHT * s).ceil() as i32 };
        // SAFETY: valid rectangle.
        let _ = unsafe { AdjustWindowRectExForDpi(&mut r, STYLE, false, WINDOW_EX_STYLE(0), dpi) };
        let (w, full) = (r.right - r.left, r.bottom - r.top);
        let h = full.min(max_h.max(1));
        // The title bar and borders stay; the content gives way.
        let content = ((HEIGHT * s).ceil() as i32 - (full - h)).max(1);
        VIEW_H.store((content as f32 / s).min(HEIGHT).to_bits(), std::sync::atomic::Ordering::Relaxed);
        (w, h)
    }

    /// The overview page for the period last shown.
    pub fn overview(&self) -> Page {
        Page::Overview(self.period)
    }

    /// Opens the window on `page`, centred on the monitor of `anchor`; the
    /// caller computes the page's data.
    pub fn open(&mut self, model: Model, page: Page, anchor: RECT, dpi: u32) {
        if self.hwnd.is_none() {
            // SAFETY: plain window creation with a registered class.
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    CLASS,
                    w!("Trayward Statistics"),
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
        // SAFETY: our own window; the title is copied.
        let _ = unsafe { SetWindowTextW(hwnd, &HSTRING::from(t("Trayward Statistics"))) };
        let was_visible = self.visible();
        self.history.clear();
        // A search left from last time would stay, unseen, on pages without a search field.
        self.search.clear();
        self.page = page;
        self.loading();
        self.scroll = 0.0;
        self.model = None;
        self.set_model(model);
        // SAFETY: Win32 calls on our own window with valid out-pointers.
        unsafe {
            if !was_visible {
                let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST), &mut mi);
                let work = mi.rcWork;
                let (w, h) = Self::window_size(dpi, work.bottom - work.top);
                let x = (work.left + ((work.right - work.left) - w) / 2).max(work.left);
                let y = (work.top + ((work.bottom - work.top) - h) / 2).max(work.top);
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
        // Lists can be large; they're computed again when the window opens.
        self.data = Data::Loading;
        new_data();
        self.loaded();
        self.layout = Layout::default();
    }

    fn view(&self) -> Option<View<'_>> {
        Some(View {
            m: self.model.as_ref()?,
            page: &self.page,
            data: &self.data,
            period: self.period,
            back: !self.history.is_empty(),
            hover: self.hover,
            mouse: self.mouse,
            scroll: self.scroll,
            search: &self.search,
            order: self.order,
            copied: self.copied.as_deref(),
            progress: {
                use std::sync::atomic::Ordering::Relaxed;
                (self.progress.done.load(Relaxed), self.progress.total.load(Relaxed))
            },
        })
    }

    fn invalidate(&self) {
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window.
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        }
    }

    fn relayout(&mut self) {
        let layout = self.view().map(|v| render(None, &View { hover: None, ..v }).unwrap_or_default());
        if let Some(l) = layout {
            self.layout = l;
            let max = (self.layout.content_h - view_h()).max(0.0);
            self.scroll = self.scroll.min(max);
            self.target = self.target.min(max);
        }
        self.update_hover();
        self.invalidate();
    }

    /// The language changed: the title, and the layout (the texts' widths change).
    pub fn retranslate(&mut self) {
        let Some(hwnd) = self.hwnd else { return };
        // SAFETY: our own window; the title is copied.
        let _ = unsafe { SetWindowTextW(hwnd, &HSTRING::from(t("Trayward Statistics"))) };
        if self.visible() {
            self.relayout();
        }
    }

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
        self.model = Some(model);
        self.relayout();
    }

    /// Shows computed statistics (ignored if they no longer fit the page).
    pub fn set_data(&mut self, key: &Page, data: Data) {
        if *key == self.page.data_key() {
            self.data = data;
            new_data();
            self.loaded();
            self.relayout();
        }
    }

    /// Switches to `page`; returns the page to compute if it needs new data.
    fn go(&mut self, page: Page, reload: bool) -> Option<Page> {
        let reload = reload || page.data_key() != self.page.data_key() || self.data == Data::Loading;
        if let Page::Overview(p) = &page {
            self.period = *p;
        }
        if page.data_key() != self.page.data_key() {
            self.search.clear();
        }
        self.page = page;
        self.scroll = 0.0;
        self.target = 0.0;
        if reload {
            self.loading();
        }
        self.relayout();
        reload.then(|| self.page.data_key())
    }

    /// Handles a click at client pixel (x, y).
    pub fn click(&mut self, x: i32, y: i32) -> Option<Command> {
        let i = self.find(self.dips(x, y)?)?;
        match self.layout.hits[i].action.clone() {
            Action::Go(p) => {
                self.history.push(self.page.clone());
                self.go(p, false).map(Command::Load)
            }
            Action::Nav(p) => {
                self.history.clear();
                // Clicking the current entry again reads the transcripts again.
                self.go(p, true).map(Command::Load)
            }
            Action::Swap(p) => self.go(p, false).map(Command::Load),
            Action::Back => self.back(),
            Action::Open(p) => Some(Command::Open(p)),
            Action::Reveal(p) => Some(Command::Reveal(p)),
            Action::Export => {
                let now = self.model.as_ref().map_or(0, |m| m.now);
                export(&self.page, &self.data, &self.search, &self.order, now)
                    .map(|(name, csv)| Command::Export { name, csv })
            }
            Action::SortSessions(s) => {
                let o = &mut self.order;
                // The selected one again turns the direction; titles start A to Z.
                o.sessions_desc = if o.sessions == s { !o.sessions_desc } else { s != SessionSort::Title };
                o.sessions = s;
                self.relayout();
                None
            }
            Action::SortFiles(s) => {
                let o = &mut self.order;
                o.files_desc = if o.files == s { !o.files_desc } else { s != FileSort::Name };
                o.files = s;
                self.relayout();
                None
            }
            Action::SortProjects(s) => {
                let o = &mut self.order;
                o.projects_desc = if o.projects == s { !o.projects_desc } else { s != ProjectSort::Name };
                o.projects = s;
                self.relayout();
                None
            }
            Action::CallsOldestFirst(oldest) => {
                self.order.calls_oldest_first = oldest;
                self.relayout();
                None
            }
            Action::AllAgents(all) => {
                self.order.all_agents = all;
                self.relayout();
                None
            }
            Action::Copy(text) => {
                self.copied = Some(text.clone());
                self.relayout();
                Some(Command::Copy(text))
            }
            Action::ClearSearch => {
                self.clear_search();
                None
            }
            Action::Hover => None,
        }
    }

    fn searchable(&self) -> bool {
        matches!(self.page, Page::Calls { .. } | Page::Files { .. } | Page::Sessions { .. })
    }

    fn set_search(&mut self, text: String) {
        self.search = text;
        self.scroll = 0.0;
        self.target = 0.0;
        self.relayout();
    }

    /// Typing on a list page searches it.
    pub fn type_text(&mut self, text: &str) {
        // The search is one line of this length at most (the clipboard can hold a book,
        // which would be laid out for every frame).
        let room = SEARCH_CHARS.saturating_sub(self.search.chars().count());
        let text: String = text.chars().filter(|c| !c.is_control()).take(room).collect();
        if self.searchable() && !text.is_empty() {
            self.set_search(self.search.clone() + &text);
        }
    }

    /// Backspace: deletes from the search; `false` if there was nothing to delete.
    pub fn backspace(&mut self) -> bool {
        let mut s = self.search.clone();
        if s.pop().is_none() {
            return false;
        }
        self.set_search(s);
        true
    }

    /// Esc: clears the search; `false` if it was empty.
    pub fn clear_search(&mut self) -> bool {
        if self.search.is_empty() {
            return false;
        }
        self.set_search(String::new());
        true
    }

    pub fn back(&mut self) -> Option<Command> {
        let p = self.history.pop()?;
        self.go(p, false).map(Command::Load)
    }

    /// Scrolls by `dy` DIPs (positive: down), gliding there.
    pub fn scroll_by(&mut self, dy: f32) {
        let max = (self.layout.content_h - view_h()).max(0.0);
        let to = (self.target + dy).clamp(0.0, max);
        if to != self.target {
            self.target = to;
            if let Some(hwnd) = self.hwnd {
                // SAFETY: our own window; the timer calls `tick` until there.
                unsafe { SetTimer(Some(hwnd), SCROLL_TIMER, 15, None) };
            }
        }
    }

    /// One step of smooth scrolling: a third of the way per frame, so it
    /// starts fast and settles softly.
    pub fn tick(&mut self) {
        let left = self.target - self.scroll;
        if left.abs() < 0.75 {
            self.scroll = self.target;
            if let Some(hwnd) = self.hwnd {
                // SAFETY: our own window and timer.
                let _ = unsafe { KillTimer(Some(hwnd), SCROLL_TIMER) };
            }
        } else {
            self.scroll += left * 0.33;
        }
        self.update_hover();
        self.invalidate();
    }

    /// Scrolls by mouse-wheel `delta` (120 per notch, positive: up).
    pub fn wheel(&mut self, delta: i16) {
        self.scroll_by(-f32::from(delta) / 120.0 * WHEEL_STEP);
    }

    pub fn page_height() -> f32 {
        view_h() - 80.0
    }

    fn dips(&self, x: i32, y: i32) -> Option<(f32, f32)> {
        let s = scale_of(self.hwnd?);
        Some((x as f32 / s, y as f32 / s))
    }

    fn find(&self, (x, y): (f32, f32)) -> Option<usize> {
        self.layout.hits.iter().position(|h| {
            let scrolls = h.tag;
            let y = if scrolls { y + self.scroll } else { y };
            let inside_page = !scrolls || x >= NAV_W;
            inside_page && h.contains(x, y)
        })
    }

    fn update_hover(&mut self) {
        self.hover = self.mouse.and_then(|p| self.find(p));
    }

    pub fn set_mouse(&mut self, pos: Option<(i32, i32)>) {
        self.mouse = pos.and_then(|(x, y)| self.dips(x, y));
        let old = self.hover;
        self.update_hover();
        let page_mouse = self.mouse.filter(|&(x, _)| x >= NAV_W).map(|(x, y)| (x, y + self.scroll));
        let in_zone = page_mouse.is_some_and(|(mx, my)| {
            self.layout.zones.iter().any(|z| mx >= z[0] && mx < z[0] + z[2] && my >= z[1] && my < z[1] + z[3])
        });
        if old != self.hover || in_zone || self.in_zone {
            self.invalidate();
        }
        self.in_zone = in_zone;
    }

    /// Whether the mouse is on something clickable.
    pub fn hovering(&self) -> bool {
        self.hover.is_some_and(|i| self.layout.hits.get(i).is_some_and(|h| h.action != Action::Hover))
    }

    /// Handles WM_PAINT.
    pub fn paint(&self, hwnd: HWND) {
        // SAFETY: BeginPaint/EndPaint pair on our own window.
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            if let Some(v) = self.view() {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let pal = theme::palette(v.m.dark);
                let _ = gfx::paint_window(hwnd, rc.right, rc.bottom, 96.0 * scale_of(hwnd), pal.window, |c| {
                    let _ = render(Some(c), &v);
                });
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }
}

/// Whether `path` is one Explorer may be shown: a whole path on a drive or in a
/// WSL distribution. Paths come from transcripts (a tool call's file, even one that
/// failed), so anything else is left alone: a relative name or an address would be
/// run or opened by Explorer, and another PC's share would be logged in to just by
/// looking at it.
fn explorable(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    let mut parts = path.components();
    let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) = (parts.next(), parts.next()) else {
        return false;
    };
    match prefix.kind() {
        Prefix::Disk(_) => true,
        Prefix::UNC(server, _) => server.eq_ignore_ascii_case("wsl.localhost") || server.eq_ignore_ascii_case("wsl$"),
        _ => false,
    }
}

/// Opens a folder, or shows a file, in Explorer. Explorer only ever gets a folder
/// that exists (a file it's given, it opens: a program would run), or a file that
/// exists to select in its folder.
pub fn explore(path: &Path, select: bool) {
    use std::os::windows::process::CommandExt;
    if !explorable(path) {
        return;
    }
    let path = path.to_path_buf();
    // On a thread: it looks at the disk (which can be WSL's) and starts a program.
    std::thread::spawn(move || {
        let Some(explorer) = crate::win::system_program("explorer.exe") else { return };
        let mut cmd = std::process::Command::new(explorer);
        if select && path.exists() {
            cmd.raw_arg(format!("/select,\"{}\"", path.display()));
        } else {
            // The file is gone (or was never there): the nearest folder that exists.
            let Some(dir) = path.ancestors().find(|p| p.is_dir()) else { return };
            cmd.arg(dir);
        }
        let _ = cmd.spawn();
    });
}

// German isn't spell-checked (cspell checks English).
// cspell:disable
/// German translations of this window's text (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    // Window and navigation.
    ("Trayward Statistics", "Trayward-Statistik"),
    ("Statistics", "Statistik"),
    ("Overview", "Übersicht"),
    ("All sessions", "Alle Sitzungen"),
    ("Storage", "Speicher"),
    ("Weeks", "Wochen"),
    ("{0} week", "{0} Woche"),
    ("{0} weeks", "{0} Wochen"),
    ("Monday to Sunday; a week's numbers are saved once it's over", "Montag bis Sonntag; die Zahlen einer Woche werden gespeichert, sobald sie vorbei ist"),
    ("This week", "Diese Woche"),
    ("Last week", "Letzte Woche"),
    ("Claude worked", "Claude arbeitete"),
    ("Active sessions", "Aktive Sitzungen"),
    ("Back", "Zurück"),
    ("Reading all transcripts… the first time takes a few seconds.", "Alle Transkripte werden gelesen… beim ersten Mal dauert das ein paar Sekunden."),
    ("Reading transcripts…", "Transkripte werden gelesen…"),
    ("Reading transcripts… {0} of {1}", "Transkripte werden gelesen… {0} von {1}"),
    ("No transcript found for this session.", "Für diese Sitzung wurde kein Transkript gefunden."),
    // Periods.
    ("Today", "Heute"),
    ("7 days", "7 Tage"),
    ("30 days", "30 Tage"),
    ("All time", "Gesamt"),
    ("Since {0}", "Seit {0}"),
    ("Last {0}", "Letzte {0}"),
    ("yesterday until this time", "gestern bis zu dieser Uhrzeit"),
    ("the 7 days before", "in den 7 Tagen davor"),
    ("the 30 days before", "in den 30 Tagen davor"),
    ("= same as before", "= wie zuvor"),
    ("About the same as {0}.", "Etwa so viel wie {0}."),
    ("{0}% more than {1}.", "{0}% mehr als {1}."),
    ("{0}% less than {1}.", "{0}% weniger als {1}."),
    // Counts.
    ("{0}d {1}h", "{0}T {1}h"),
    ("{0} session", "{0} Sitzung"),
    ("{0} sessions", "{0} Sitzungen"),
    ("in {0} project", "in {0} Projekt"),
    ("in {0} projects", "in {0} Projekten"),
    ("{0} active hour", "{0} aktive Stunde"),
    ("{0} active hours", "{0} aktive Stunden"),
    ("{0} prompt", "{0} Prompt"),
    ("{0} prompts", "{0} Prompts"),
    ("{0} reply", "{0} Antwort"),
    ("{0} replies", "{0} Antworten"),
    ("{0} tool call", "{0} Tool-Aufruf"),
    ("{0} tool calls", "{0} Tool-Aufrufe"),
    ("{0} failed", "{0} fehlgeschlagen"),
    ("{0} failed call", "{0} Fehlschlag"),
    ("{0} failed calls", "{0} Fehlschläge"),
    ("{0} file", "{0} Datei"),
    ("{0} files", "{0} Dateien"),
    ("{0} edit", "{0}× geändert"),
    ("{0} edits", "{0}× geändert"),
    ("{0} read", "{0}× gelesen"),
    ("{0} reads", "{0}× gelesen"),
    ("{0} tokens written", "{0} Token erzeugt"),
    ("{0} tokens written ({1} by agents)", "{0} Token erzeugt ({1} von Agenten)"),
    ("≈ {0} at API prices", "≈ {0} zu API-Preisen"),
    ("Week of {0}", "Woche vom {0}"),
    // Tiles.
    ("prompts", "Prompts"),
    ("replies", "Antworten"),
    ("tool calls", "Tool-Aufrufe"),
    ("agents started", "Subagenten"),
    ("Subagents Claude started with the Agent tool.", "Subagenten, die Claude mit dem Agent-Tool gestartet hat."),
    ("tokens written", "Token erzeugt"),
    ("{0} by the session itself, {1} by subagents. {2} of it was thinking.", "{0} von der Sitzung selbst, {1} von Subagenten. {2} davon war Nachdenken."),
    ("from cache", "aus dem Cache"),
    ("{0} of {1} input tokens were read from the prompt cache, which is cheaper and faster.", "{0} von {1} Eingabe-Token kamen aus dem Prompt-Cache – das ist günstiger und schneller."),
    ("sessions", "Sitzungen"),
    ("failed calls", "Fehlschläge"),
    ("files changed", "Dateien"),
    ("lines written", "neue Zeilen"),
    ("{0} lines written and {1} lines replaced by edits.", "{0} Zeilen geschrieben und {1} Zeilen durch Änderungen ersetzt."),
    ("commits", "Commits"),
    ("Successful git commit commands.", "Erfolgreiche git-commit-Befehle."),
    ("web lookups", "Web-Abfragen"),
    ("Web searches and page fetches.", "Websuchen und abgerufene Seiten."),
    ("API value", "API-Wert"),
    ("What these tokens would cost at Anthropic's API prices ({0} of it by subagents). Your plan is billed differently.", "Was diese Token zu den API-Preisen von Anthropic kosten würden ({0} davon durch Subagenten). Dein Abo wird anders abgerechnet."),
    ("turns", "Runden"),
    ("Prompts that were worked on.", "Prompts, an denen gearbeitet wurde."),
    ("time working", "Zeit aktiv"),
    ("From each prompt to its last step, added up. Includes time spent waiting for your answers.", "Von jedem Prompt bis zu seinem letzten Schritt, zusammengezählt. Inklusive der Zeit, in der auf deine Antworten gewartet wurde."),
    ("median turn", "Median-Runde"),
    ("Half of the turns were shorter than this.", "Die Hälfte der Runden war kürzer."),
    ("longest turn", "längste Runde"),
    ("compactions", "Verdichtungen"),
    ("Times the conversation was summarised to make room in the context.", "Wie oft das Gespräch zusammengefasst wurde, um Platz im Kontext zu schaffen."),
    // Cards and charts.
    ("Tokens written per hour", "Erzeugte Token pro Stunde"),
    ("Tokens written per day", "Erzeugte Token pro Tag"),
    ("Tokens written per week", "Erzeugte Token pro Woche"),
    ("incl. agents", "inkl. Agenten"),
    ("Projects", "Projekte"),
    ("incl. agents · at API prices", "inkl. Agenten · zu API-Preisen"),
    ("share", "Anteil"),
    ("written", "geschrieben"),
    ("read", "gelesen"),
    ("last used {0}", "zuletzt {0}"),
    ("since {0}", "seit {0}"),
    ("Tools", "Tools"),
    ("All calls", "Alle Aufrufe"),
    ("When you work", "Wann du arbeitest"),
    ("prompts and replies by weekday and hour, local time", "Prompts und Antworten nach Wochentag und Uhrzeit, Ortszeit"),
    ("Nothing", "Nichts"),
    ("1 prompt or reply", "1 Prompt oder Antwort"),
    ("{0} prompts and replies", "{0} Prompts und Antworten"),
    ("Commands", "Befehle"),
    ("programs run in the shell", "in der Shell gestartete Programme"),
    ("Models", "Modelle"),
    ("share of tokens written", "Anteil an erzeugten Token"),
    ("Top sessions", "Top-Sitzungen"),
    ("by tokens written", "nach erzeugten Token"),
    ("Nothing yet.", "Noch nichts."),
    ("Longest turns", "Längste Runden"),
    ("from a prompt to its last step", "vom Prompt bis zu seinem letzten Schritt"),
    ("Turn of {0}", "Runde von {0}"),
    ("Common failures", "Häufige Fehler"),
    ("alike errors grouped", "ähnliche Fehler zusammengefasst"),
    ("All failures", "Alle Fehler"),
    ("last {0}", "zuletzt {0}"),
    ("Error: {0}", "Fehler: {0}"),
    ("Declined or blocked: {0}", "Abgelehnt oder blockiert: {0}"),
    // Weekly limit.
    ("Weekly limit", "Wochenlimit"),
    ("{0}% now", "jetzt {0}%"),
    ("last week", "letzte Woche"),
    ("{0} weeks ago", "vor {0} Wochen"),
    ("last week ended at {0}%", "letzte Woche endete bei {0}%"),
    ("No weekly limit data yet.", "Noch keine Daten zum Wochenlimit."),
    ("{0}% used", "{0}% verbraucht"),
    ("{0}% at an even pace", "{0}% bei gleichmäßigem Tempo"),
    ("diagonal: even pace over the week", "Diagonale: gleichmäßiges Tempo über die Woche"),
    ("diagonal: even pace · grey: earlier weeks", "Diagonale: gleichmäßiges Tempo · grau: frühere Wochen"),
    ("resets {0}", "Reset {0}"),
    // A session.
    ("tokens", "Token"),
    ("size", "Größe"),
    ("archived", "archiviert"),
    ("started {0}", "gestartet {0}"),
    ("active over {0}", "aktiv über {0}"),
    ("Copied", "Kopiert"),
    ("Resume command", "Fortsetzen-Befehl"),
    ("Transcripts", "Transkripte"),
    ("Project", "Projekt"),
    ("Context size", "Kontextgröße"),
    ("peak {0}", "Spitze {0}"),
    ("peak {0} of {1}", "Spitze {0} von {1}"),
    ("Starting context", "Kontext beim Start"),
    ("Agents", "Agenten"),
    ("Agent map", "Agentenbaum"),
    ("This session started none.", "Diese Sitzung hat keine gestartet."),
    ("Started", "Gestartet"),
    ("Types", "Arten"),
    ("Ran for", "Laufzeit"),
    ("{0} in total", "{0} insgesamt"),
    ("Longest", "Längster"),
    ("Tokens written", "Token erzeugt"),
    ("Show all", "Alle zeigen"),
    ("Show {0} more", "{0} weitere zeigen"),
    ("{0} agent", "{0} Agent"),
    ("{0} running", "{0} laufen"),
    ("{0} · {1} in context", "{0} · {1} im Kontext"),
    ("{0} · {1} written · {2} calls", "{0} · {1} geschrieben · {2} Aufrufe"),
    ("Type: {0}", "Typ: {0}"),
    ("Model: {0}", "Modell: {0}"),
    ("{0} tokens written, {1} in context at the end", "{0} Tokens geschrieben, am Ende {1} im Kontext"),
    ("{0} tool calls, {1} failed", "{0} Tool-Aufrufe, {1} fehlgeschlagen"),
    ("API value: {0}", "API-Wert: {0}"),
    ("Started {0}, ran {1}", "Gestartet {0}, lief {1}"),
    ("Still running", "Läuft noch"),
    ("Finished", "Fertig"),
    ("median {0}", "Median {0}"),
    ("{0} before the first reply", "{0} vor der ersten Antwort"),
    ("{0} tokens before the first reply", "{0} Tokens vor der ersten Antwort"),
    ("Not in the transcripts of this period.", "Nicht in den Transkripten dieses Zeitraums."),
    ("Not in this transcript.", "Nicht in diesem Transkript."),
    ("Setup", "Ausstattung"),
    ("as of the latest session", "Stand der letzten Sitzung"),
    ("Skills", "Skills"),
    ("MCP servers", "MCP-Server"),
    ("{0} · {1} tools", "{0} · {1} Tools"),
    ("Agent types", "Agent-Typen"),
    ("Instruction files", "Anweisungsdateien"),
    ("Memory index", "Memory-Index"),
    ("loaded", "geladen"),
    ("not loaded", "nicht geladen"),
    ("Memory files", "Memory-Dateien"),
    ("{0} in all projects", "{0} in allen Projekten"),
    ("{0} in this project", "{0} in diesem Projekt"),
    ("System prompt", "System-Prompt"),
    ("CLAUDE.md and rules", "CLAUDE.md und Regeln"),
    ("Memory", "Memory"),
    ("Tools and the rest", "Tools und der Rest"),
    ("{0} tokens of context", "{0} Token Kontext"),
    ("{0} on disk", "{0} belegt"),
    ("Files changed", "Geänderte Dateien"),
    ("All files", "Alle Dateien"),
    ("Tokens", "Token"),
    ("{0} thinking", "{0} zum Nachdenken"),
    ("Cache read", "Cache gelesen"),
    ("Cache write", "Cache geschrieben"),
    ("Input", "Eingabe"),
    ("Output", "Ausgabe"),
    ("By agents", "Von Agenten"),
    // Lists.
    ("Type to search", "Tippen zum Suchen"),
    ("Sort by", "Sortieren nach"),
    ("Order", "Reihenfolge"),
    ("Newest first", "Neueste zuerst"),
    ("Oldest first", "Älteste zuerst"),
    ("Nothing here.", "Hier ist nichts."),
    // The session page's storage, named in stats.rs.
    ("Transcript", "Transkript"),
    ("Subagent transcripts", "Transkripte der Subagenten"),
    ("Saved tool output", "Gespeicherte Werkzeugausgaben"),
    ("File history", "Dateiverlauf"),
    ("Session environment", "Sitzungsumgebung"),
    ("Kept statistics", "Behaltene Statistik"),
    ("Showing the first {0} of {1}. Search or filter to narrow it down; the CSV has all.", "Die ersten {0} von {1}. Suche oder Filter grenzen ein; die CSV-Datei enthält alle."),
    ("All  {0}", "Alle  {0}"),
    ("Failed  {0}", "Fehlgeschlagen  {0}"),
    ("Not archived  {0}", "Nicht archiviert  {0}"),
    ("Archived  {0}", "Archiviert  {0}"),
    ("Tool calls", "Tool-Aufrufe"),
    ("Failed tool calls", "Fehlgeschlagene Tool-Aufrufe"),
    ("{0} calls", "{0}-Aufrufe"),
    ("Failures like this", "Ähnliche Fehler"),
    ("click one to list only those", "klick einen an, um nur diese zu sehen"),
    ("Files", "Dateien"),
    ("{0} changed", "{0} geändert"),
    ("{0} only read", "{0} nur gelesen"),
    ("No files read or changed.", "Keine Dateien gelesen oder geändert."),
    ("past and archived sessions included", "inkl. früherer und archivierter Sitzungen"),
    ("Recent", "Zuletzt"),
    ("Prompts", "Prompts"),
    ("Size", "Größe"),
    ("Duration", "Dauer"),
    ("Failures", "Fehler"),
    ("Title", "Titel"),
    ("Edits", "Änderungen"),
    ("Lines", "Zeilen"),
    ("Reads", "Lesezugriffe"),
    ("Name", "Name"),
    // Storage.
    ("{0} in {1} · Claude Code's own folders", "{0} in {1} · Ordner von Claude Code"),
    ("Open", "Öffnen"),
    ("Transcripts per project · {0}", "Transkripte pro Projekt · {0}"),
    ("largest first", "größte zuerst"),
    ("File history (undo)", "Dateiverlauf (Rückgängig)"),
    ("Shell snapshots", "Shell-Snapshots"),
    ("Session environments", "Sitzungsumgebungen"),
    ("Plugins", "Plugins"),
    ("Skills", "Skills"),
    ("To-do lists", "To-do-Listen"),
    ("Plans", "Pläne"),
    ("Cache", "Cache"),
    ("Backups", "Backups"),
    ("Debug logs", "Debug-Logs"),
    ("Feature flags", "Feature-Flags"),
    ("IDE connections", "IDE-Verbindungen"),
    ("Running sessions", "Laufende Sitzungen"),
    ("Prompt history", "Prompt-Verlauf"),
    ("Settings", "Einstellungen"),
    ("transcript deleted, statistics kept", "Transkript gelöscht, Statistik behalten"),
];
// cspell:enable

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_only_gets_whole_local_paths() {
        for ok in [r"C:\dev\app\src\main.rs", r"d:\", r"\\wsl.localhost\Ubuntu\home\me\app", r"\\wsl$\Ubuntu\home"] {
            assert!(explorable(Path::new(ok)), "{ok}");
        }
        // A program's name, an address, another PC's share, a path without its drive.
        for bad in [
            "calc.exe",
            r"tools\setup.cmd\x",
            "https://example.com/a/x",
            "ms-settings:privacy/x",
            r"\\host\share\setup.exe\x",
            r"\dev\app",
            "C:app",
            "/home/me/app",
            "",
        ] {
            assert!(!explorable(Path::new(bad)), "{bad}");
        }
    }

    #[test]
    fn quotes_a_folder_for_the_shell_it_is_pasted_into() {
        assert_eq!(shell_word(r"C:\dev\app", false), r"C:\dev\app");
        assert_eq!(shell_word("/home/me/app", true), "/home/me/app");
        // Nothing in it is expanded or run.
        assert_eq!(shell_word(r"C:\dev\my app $(calc)", false), r"'C:\dev\my app $(calc)'");
        assert_eq!(shell_word("/home/me/$(reboot) `x`", true), "'/home/me/$(reboot) `x`'");
        assert_eq!(shell_word(r"C:\dev\it's", false), r"'C:\dev\it''s'");
        assert_eq!(shell_word("/home/me/it's", true), r"'/home/me/it'\''s'");
        assert_eq!(shell_word(r"C:\dev\a;b", false), r"'C:\dev\a;b'");
        assert_eq!(shell_word("", false), "''");
    }

    #[test]
    fn normalizes_points() {
        let pts = normalize(&[(0, 0.0), (50, 50.0), (100, 200.0)], 0, 100, 100.0);
        assert_eq!(pts, vec![(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]);
    }

    #[test]
    fn formats_durations() {
        crate::i18n::set(crate::i18n::Choice::English);
        assert_eq!(duration(90), "1m");
        assert_eq!(duration(3 * 3600 + 5 * 60), "3h 05m");
        assert_eq!(duration(2 * 86_400 + 3600), "2d 1h");
    }

    #[test]
    fn filters_share_data() {
        let a = Page::Calls { session: Some("s".into()), period: Period::All, filter: Filter::Failed };
        let b = Page::Calls { session: Some("s".into()), period: Period::All, filter: Filter::Tool("Bash".into()) };
        assert_eq!(a.data_key(), b.data_key());
        assert_ne!(a.data_key(), Page::Session("s".into()));
        assert!(a.under(&Page::Session("s".into())));
        assert!(!a.under(&Page::Session("t".into())));
        assert!(Page::Files { session: None, period: Period::Week }.under(&Page::Overview(Period::Today)));
    }

    #[test]
    fn splits_paths() {
        assert_eq!(file_name("C:\\a\\b.rs"), "b.rs");
        assert_eq!(parent("C:\\a\\b.rs"), "C:\\a");
        assert_eq!(file_name("/home/x/y.txt"), "y.txt");
        assert_eq!(parent("y.txt"), "");
    }

    #[test]
    fn tells_files_of_the_same_name_apart() {
        let names = distinct_names(&[r"C:\p\src\ui\mod.rs", r"C:\p\src\core\mod.rs", "/home/u/p/README.md", "a.rs"]);
        assert_eq!(names, ["ui/mod.rs", "core/mod.rs", "README.md", "a.rs"]);
        // The same path twice can't be told apart: as far as it goes, and no endless loop.
        assert_eq!(distinct_names(&["x/a.rs", "x/a.rs"]), ["x/a.rs", "x/a.rs"]);
        assert!(distinct_names(&[]).is_empty());
    }

    #[test]
    fn keeps_what_was_worked_out_until_the_data_changes() {
        let made = Cell::new(0);
        let make = || {
            made.set(made.get() + 1);
            made.get()
        };
        assert_eq!(*memo_at(7, &("calls", "a"), make), 1);
        assert_eq!(*memo_at(7, &("calls", "a"), make), 1);
        // Another filter, another kind of value, new data: worked out again.
        assert_eq!(*memo_at(7, &("calls", "b"), make), 2);
        assert_eq!(*memo_at(7, &("calls", "a"), || "text"), "text");
        assert_eq!(*memo_at(8, &("calls", "a"), make), 3);
        assert_eq!(made.get(), 3);
    }

    #[test]
    fn writes_csv() {
        assert_eq!(csv_field("plain"), "plain");
        // Not a formula for a spreadsheet; a negative number stays one.
        assert_eq!(csv_field("=cmd|' /C calc'!A0"), "'=cmd|' /C calc'!A0");
        assert_eq!(csv_field("@file, please"), "\"'@file, please\"");
        assert_eq!(csv_field("-12.5"), "-12.5");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        let text = csv(&["a", "b"], [vec!["1".to_string(), "x,y".to_string()]].into_iter());
        assert_eq!(text, "\u{FEFF}a,b\r\n1,\"x,y\"\r\n");
        assert!(matches("", &["x"]));
        assert!(matches("CARGO", &["git", "cargo build"]));
        assert!(!matches("npm", &["git"]));
    }

    #[test]
    fn sorts_projects() {
        let p = |name: &str, sessions: u32, cost: f64, last: i64| stats::ProjectInfo {
            name: name.into(),
            sessions,
            cost,
            last_at: Some(last),
            ..Default::default()
        };
        let list = [p("beta", 5, 1.0, 10), p("Alpha", 9, 1.0, 5), p("gamma", 1, 7.5, 20)];
        let names = |order: Order| project_rows(&list, &order).iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        // As before there was a choice: the one used last first.
        assert_eq!(names(Order::default()), ["gamma", "beta", "Alpha"]);
        let by = |projects, projects_desc| Order { projects, projects_desc, ..Order::default() };
        assert_eq!(names(by(ProjectSort::Sessions, true)), ["Alpha", "beta", "gamma"]);
        assert_eq!(names(by(ProjectSort::Sessions, false)), ["gamma", "beta", "Alpha"]);
        assert_eq!(names(by(ProjectSort::Name, false)), ["Alpha", "beta", "gamma"]);
        // Equal ones: the one used last first, either way.
        assert_eq!(names(by(ProjectSort::Cost, true)), ["gamma", "beta", "Alpha"]);
        assert_eq!(names(by(ProjectSort::Cost, false)), ["beta", "Alpha", "gamma"]);
    }

    #[test]
    fn sorts_sessions() {
        let s = |id: &str, prompts: u32, last: i64, title: &str| SessionInfo {
            id: id.into(),
            title: title.into(),
            project: "p".into(),
            distro: None,
            first_at: Some(0),
            last_at: Some(last),
            prompts,
            output: 0,
            errors: 0,
            cost: 0.0,
            size: stats::Size::default(),
            archived: false,
            kept: false,
        };
        let list = [s("a", 5, 10, "beta"), s("b", 9, 5, "Alpha"), s("c", 1, 20, "gamma")];
        let ids = |order: Order| session_rows(&list, None, "", &order).iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(Order::default()), ["c", "a", "b"]);
        let by = |sessions, sessions_desc| Order { sessions, sessions_desc, ..Order::default() };
        assert_eq!(ids(by(SessionSort::Prompts, true)), ["b", "a", "c"]);
        assert_eq!(ids(by(SessionSort::Prompts, false)), ["c", "a", "b"]);
        assert_eq!(ids(by(SessionSort::Title, false)), ["b", "a", "c"]);
    }

    #[test]
    fn compares_periods() {
        assert_eq!(change(120.0, 100.0), Some(20.0));
        assert_eq!(change(50.0, 100.0), Some(-50.0));
        assert_eq!(change(5.0, 0.0), None);
    }

    #[test]
    fn formats_turns() {
        assert_eq!(turn_time(45), "45s");
        assert_eq!(turn_time(190), "3m 10s");
        assert_eq!(turn_time(3900), "1h 05m");
    }

    #[test]
    fn counts_compactly() {
        assert_eq!(num(9999), "9999");
        assert_eq!(num(12_345), "12k");
        crate::i18n::set(crate::i18n::Choice::English);
        assert_eq!(plural(1, "{0} file", "{0} files"), "1 file");
        assert_eq!(plural(2, "{0} file", "{0} files"), "2 files");
    }
}
